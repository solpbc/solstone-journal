// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Explicit owner-confirmed iOS teardown; routine retention remains refused.

use super::*;
use solstone_core_backup::ios_repository as repository;
use solstone_core_backup::teardown_configuration::TeardownConfiguration;
use solstone_core_backup::teardown_recovery::retained::{Mode, Record, Store};
use solstone_core_backup_runtime::ios_callable;
use solstone_core_backup_runtime::ios_callable::teardown::{
    DestinationContext, DestructiveAdmission, LockedDestination,
};
use solstone_core_backup_runtime::teardown_admission::{TeardownReader, admit_ios_configuration};
use solstone_core_journal_config_write::BoundConfigOutcome;
use solstone_core_journal_io::media::teardown_preparation::TeardownPreparation;
use solstone_core_journal_io::media::{
    ActivityCarrierRetirementEffect, Confirmation, Durability, MediaDirectory,
    RootCarrierObservation,
};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Actual native and record owners survive returned refusal and unwind. Their
/// serialized observations alone are never exposed as executing authorities.
#[derive(Default)]
pub(super) struct Custody {
    preparation: Option<TeardownPreparation>,
    context: Option<Arc<DestinationContext>>,
    native: Option<LockedDestination>,
    store: Option<Store>,
    finished_configuration: Option<TeardownConfiguration>,
}

pub(super) fn confirmed_restart_custody(held: &HeldTeardown) -> Result<(), ()> {
    if held.phase != TeardownRunPhase::Clear
        || !held.outside.attempts.clear
        || held.outside.attempts.binding
        || held.outside.attempts.retirement
        || held.outside.binding.is_some()
        || held.outside.retirement.is_some()
    {
        return Err(());
    }
    let Some(PhaseOwner::Cleared(cleared)) = &held.owner else {
        return Err(());
    };
    let outside = &held.outside;
    let store = outside.ios.store.as_ref().ok_or(())?;
    let preparation = outside.ios.preparation.as_ref().ok_or(())?;
    let native = outside.ios.native.as_ref().ok_or(())?;
    let publication = outside.publication.as_ref().ok_or(())?;
    let carrier = outside.carrier.as_ref().ok_or(())?;
    if !Arc::ptr_eq(publication, cleared.publication_receipt())
        || !Arc::ptr_eq(carrier, cleared.carrier())
        || outside
            .lease
            .as_ref()
            .is_none_or(|lease| !Arc::ptr_eq(lease, cleared.teardown_lease()))
        || outside
            .root
            .as_ref()
            .is_none_or(|root| !Arc::ptr_eq(root, cleared.journal_root()))
        || store.record().native.as_ref() != Some(native.observation())
        || outside.operation.as_ref().and_then(|op| {
            op.report()
                .confirmed_facts()
                .or_else(|| op.report().previous().cloned())
        }) != store.record().remote_facts
    {
        return Err(());
    }
    // No fresh budget or cancellation reset: these checks observe original
    // descriptors and durable bytes without issuing a remote/local effect.
    native.confirm_restart_custody().map_err(|_| ())?;
    cleared.revalidate().map_err(|_| ())?;
    super::restart::confirm_local_custody(
        store,
        preparation,
        carrier,
        cleared.configuration(),
        publication,
    )?;
    native.confirm_restart_custody().map_err(|_| ())?;
    store.revalidate().map_err(|_| ())
}

fn recovery() -> TeardownRunRefusal {
    TeardownRunRefusal::RetirementUncertain
}
fn operation(outside: &Outside) -> &TeardownOperation {
    outside.operation.as_ref().expect("original operation")
}
fn native_current(outside: &Outside) -> Result<(), TeardownRunRefusal> {
    outside
        .ios
        .native
        .as_ref()
        .expect("actual locked native destination")
        .revalidate(operation(outside))
        .map_err(|_| recovery())
}
fn record(outside: &Outside) -> &Record {
    outside
        .ios
        .store
        .as_ref()
        .expect("actual recovery owner")
        .record()
}
fn update(
    outside: &mut Outside,
    change: impl FnOnce(&mut Record),
) -> Result<(), TeardownRunRefusal> {
    outside.remaining()?;
    let store = outside.ios.store.as_mut().expect("actual recovery owner");
    let mut next = store.record().clone();
    change(&mut next);
    store.update(next).map_err(|_| recovery())?;
    outside.remaining()
}
fn carrier(owner: &Option<PhaseOwner>) -> &CarrierTeardownAdmission {
    match owner.as_ref().expect("live phase") {
        PhaseOwner::Carrier(value) => value,
        _ => unreachable!("carrier phase"),
    }
}
fn coverage(
    root: &Arc<JournalRoot>,
    config: &TeardownConfiguration,
    carrier: Option<RootCarrierObservation<'_>>,
    offload: bool,
) -> Result<(), solstone_core_backup_runtime::teardown_admission::TeardownReaderRefusal> {
    use solstone_core_backup_runtime::teardown_admission::TeardownReaderRefusal;
    if offload
        && super::super::read_complete_catalog(root)
            .map_err(|_| TeardownReaderRefusal::CatalogUnavailable)?
            .days
            .is_empty()
    {
        return Err(TeardownReaderRefusal::CatalogUnavailable);
    }
    super::super::read_teardown_coverage(root, config, carrier)
}

pub fn run(
    root: &Arc<JournalRoot>,
    clock: Arc<dyn Clock + Send + Sync>,
    cancellation: ArchiveCancellation,
    version: &str,
) -> OwnedTeardownOutcome {
    let mut outside = Outside::new(clock, cancellation);
    outside.root = Some(root.clone());
    let mut owner = None;
    let mut phase = TeardownRunPhase::Admission;
    let result = catch_unwind(AssertUnwindSafe(|| {
        execute(root, &mut outside, &mut owner, &mut phase, version)
    }));
    match result {
        Err(_) => unknown(outside, owner, phase),
        Ok(Err(refusal)) => held(outside, owner, phase, refusal),
        Ok(Ok(())) => {
            let retirement = outside.retirement.take().map(|r| r.receipt);
            let report = SettledTeardownReport {
                remote: outside
                    .operation
                    .take()
                    .expect("original operation")
                    .into_report(),
                attempts: std::mem::take(&mut outside.attempts),
                publication: outside
                    .publication
                    .take()
                    .expect("actual current publication"),
                binding: outside.binding.take(),
                retirement,
            };
            drop(owner);
            drop(outside);
            OwnedTeardownOutcome::Finished(Box::new(report))
        }
    }
}

fn execute(
    root: &Arc<JournalRoot>,
    outside: &mut Outside,
    owner: &mut Option<PhaseOwner>,
    phase: &mut TeardownRunPhase,
    version: &str,
) -> Result<(), TeardownRunRefusal> {
    root.revalidate_canonical_binding()
        .map_err(TeardownRunRefusal::Root)?;
    outside.operation = Some(
        TeardownOperation::new(outside._clock.clone(), outside.cancellation.clone())
            .map_err(TeardownRunRefusal::Budget)?,
    );
    let lease = TeardownLease::try_acquire(root.clone()).map_err(|e| {
        TeardownRunRefusal::Admission(match e {
            solstone_core_journal_io::activity::ActivityAdmissionError::Busy => {
                TeardownAdmissionRefusal::ActivityBusy
            }
            _ => TeardownAdmissionRefusal::ActivityUnavailable,
        })
    })?;
    outside.lease = Some(lease.clone());
    outside.ios.store = Store::load(lease.clone()).map_err(|_| recovery())?;
    let mut discovered =
        Store::discover_preparation(lease.clone(), outside.ios.store.as_ref().map(Store::record))
            .map_err(|_| recovery())?;
    if let (Some(store), Some(found)) = (&outside.ios.store, &discovered)
        && store.record() != &found.record
    {
        return Err(recovery());
    }
    let previous = outside
        .ios
        .store
        .as_ref()
        .map(|s| s.record().clone())
        .or_else(|| discovered.as_ref().map(|found| found.record.clone()));
    if let Some(facts) = previous.as_ref().and_then(|r| r.remote_facts.clone()) {
        outside
            .operation
            .as_mut()
            .expect("original operation")
            .restore_prior_facts(facts);
    }
    let configuration = match previous.as_ref() {
        Some(record) if record.clear.is_some() => TeardownConfiguration::capture_local_finish(
            &lease,
            &record.local,
            record.clear.as_ref().expect("clear observation"),
            record.binding_removed,
        ),
        Some(record) => TeardownConfiguration::capture_original_retry(&lease, &record.local),
        None => TeardownConfiguration::capture(root),
    }
    .map_err(|r| TeardownRunRefusal::Admission(TeardownAdmissionRefusal::Configuration(r)))?;
    let offload = previous
        .as_ref()
        .map(|r| r.offload_was_recorded)
        .unwrap_or_else(|| super::super::offload_recorded(configuration.backup()));
    let mut reader =
        |r: &Arc<JournalRoot>, c: &TeardownConfiguration, k: Option<RootCarrierObservation<'_>>| {
            coverage(r, c, k, offload)
        };

    // A durably confirmed original retirement never admits a replacement carrier.
    if previous.as_ref().is_some_and(|r| r.carrier_retired) {
        outside.ios.finished_configuration = Some(configuration);
        return finish_already_retired(root, outside, &mut reader, phase);
    }
    *phase = TeardownRunPhase::Preparation;
    if configuration
        .backup()
        .get("mode")
        .and_then(serde_json::Value::as_str)
        == Some("operated")
    {
        if previous
            .as_ref()
            .is_some_and(|r| r.clear.is_none() && r.mode != Mode::Operated)
        {
            return Err(recovery());
        }
    } else if previous
        .as_ref()
        .is_some_and(|r| r.clear.is_none() && r.mode != Mode::Byo)
    {
        return Err(recovery());
    }
    if let Some(found) = discovered.take() {
        outside.ios.preparation = Some(found.preparation);
    } else if let Some(previous) = &previous {
        if !previous.preparation_ready {
            return Err(recovery());
        }
        let preparation = TeardownPreparation::reopen(lease.clone(), &previous.preparation)
            .map_err(|_| recovery())?;
        if preparation.marker().map_err(|_| recovery())?.is_some() {
            return Err(recovery());
        }
        preparation.published_directory().map_err(|_| recovery())?;
        outside.ios.preparation = Some(preparation);
    } else {
        let preparation = TeardownPreparation::create(lease.clone()).map_err(|failure| {
            let solstone_core_journal_io::media::OwnedMediaFailure { owned, .. } = *failure;
            outside.ios.preparation = owned;
            recovery()
        })?;
        let mode = if configuration
            .backup()
            .get("mode")
            .and_then(serde_json::Value::as_str)
            == Some("operated")
        {
            Mode::Operated
        } else {
            Mode::Byo
        };
        let preflight = Record::preflight(
            mode,
            configuration.local_observation().map_err(|_| recovery())?,
            offload,
            preparation.observation().clone(),
        )
        .map_err(|_| recovery())?;
        let bytes = serde_json::to_vec(&preflight).map_err(|_| recovery())?;
        outside.ios.preparation = Some(preparation);
        outside
            .ios
            .preparation
            .as_ref()
            .expect("actual preparation")
            .write_marker(&bytes)
            .map_err(|_| recovery())?;
    }
    if outside
        .ios
        .store
        .as_ref()
        .is_none_or(|s| !s.record().preparation_ready)
    {
        let preparation = outside
            .ios
            .preparation
            .as_mut()
            .expect("actual preparation");
        preparation.publish().map_err(|_| recovery())?;
        if outside.ios.store.is_none() {
            let marker = preparation
                .marker()
                .map_err(|_| recovery())?
                .ok_or_else(recovery)?;
            let preflight: Record =
                serde_json::from_slice(&marker.bytes).map_err(|_| recovery())?;
            outside.ios.store =
                Some(Store::create(lease.clone(), preflight).map_err(|_| recovery())?);
        }
        outside
            .ios
            .store
            .as_mut()
            .expect("actual canonical owner")
            .complete_preparation(preparation)
            .map_err(|_| recovery())?;
    }
    outside.remaining()?;
    outside
        .ios
        .store
        .as_ref()
        .expect("actual canonical owner")
        .confirm_carrier_ready()
        .map_err(|_| recovery())?;
    let previous = Some(record(outside).clone());
    let admitted =
        admit_ios_configuration(lease, configuration).map_err(TeardownRunRefusal::Admission)?;
    *owner = Some(PhaseOwner::Admission(Box::new(admitted)));
    let Some(PhaseOwner::Admission(admitted)) = owner.take() else {
        unreachable!()
    };
    let prepared = admitted.reacquire_carrier(&record(outside).carrier, &mut reader);
    match prepared {
        Ok(value) => {
            outside.carrier = Some(value.carrier().clone());
            *owner = Some(PhaseOwner::Carrier(Box::new(value)));
        }
        Err(value) => {
            *owner = Some(PhaseOwner::CarrierFailure(Box::new(value)));
            return Err(TeardownRunRefusal::CarrierPreparation);
        }
    }

    if previous.as_ref().is_none_or(|r| r.clear.is_none()) {
        *phase = TeardownRunPhase::Remote;
        remote(
            root,
            outside,
            owner,
            &mut reader,
            previous.as_ref(),
            version,
        )?;
    } else {
        reopen_native(root, outside, true)?;
    }
    outside.remaining()?;
    carrier(owner)
        .check(&mut reader)
        .map_err(TeardownRunRefusal::Admission)?;
    native_current(outside)?;
    *phase = TeardownRunPhase::Clear;
    outside.attempts.clear = true;
    let Some(PhaseOwner::Carrier(admitted)) = owner.take() else {
        unreachable!()
    };
    let cleared = match admitted.clear_configuration(&mut reader) {
        Ok(value) => value,
        Err(value) => {
            outside.publication = value.publication_receipt().cloned();
            *owner = Some(PhaseOwner::ClearFailure(Box::new(value)));
            return Err(TeardownRunRefusal::Clear);
        }
    };
    outside.publication = Some(cleared.publication_receipt().clone());
    let clear = cleared
        .configuration()
        .published_clear_observation(cleared.publication())
        .map_err(|_| TeardownRunRefusal::Clear)?;
    *owner = Some(PhaseOwner::Cleared(Box::new(cleared)));
    // A real equal-value retry publication changes file identity. Save that
    // exact proof before any later deletion; a publication/record gap stays held.
    update(outside, |record| record.clear = Some(clear))?;
    native_current(outside)?;
    #[cfg(feature = "test-hooks")]
    if super::consume_interrupt(root, super::OwnedTeardownInterrupt::AfterClear) {
        return Err(recovery());
    }
    super::test_cut(TeardownRunPhase::Clear);
    outside.remaining()?;
    *phase = TeardownRunPhase::Binding;
    let Some(PhaseOwner::Cleared(cleared)) = owner.take() else {
        unreachable!()
    };
    native_current(outside)?;
    let (finish, operational) = if cleared.configuration().binding().is_some() {
        outside.attempts.binding = true;
        let guarded = match cleared.begin_binding_finalization(&mut reader) {
            Ok(value) => value,
            Err(value) => {
                *owner = Some(PhaseOwner::BindingFailure(Box::new(value)));
                return Err(TeardownRunRefusal::Binding);
            }
        };
        *owner = Some(PhaseOwner::Guarded(Box::new(guarded)));
        outside.remaining()?;
        native_current(outside)?;
        let Some(PhaseOwner::Guarded(guarded)) = owner.take() else {
            unreachable!()
        };
        let bound = match guarded.remove_binding(&mut reader) {
            Ok(value) => value,
            Err(value) => {
                outside.binding = value.binding_receipt().cloned();
                *owner = Some(PhaseOwner::BindingFailure(Box::new(value)));
                return Err(TeardownRunRefusal::Binding);
            }
        };
        outside.binding = Some(bound.binding_receipt().clone());
        *owner = Some(PhaseOwner::Bound(Box::new(bound)));
        update(outside, |record| record.binding_removed = true)?;
        let Some(PhaseOwner::Bound(bound)) = owner.take() else {
            unreachable!()
        };
        match bound.into_finish(&mut reader) {
            Ok(value) => value,
            Err(value) => {
                *owner = Some(PhaseOwner::BindingFailure(Box::new(value)));
                return Err(TeardownRunRefusal::Binding);
            }
        }
    } else {
        *owner = Some(PhaseOwner::Cleared(cleared));
        update(outside, |record| record.binding_removed = true)?;
        let Some(PhaseOwner::Cleared(cleared)) = owner.take() else {
            unreachable!()
        };
        match cleared.into_finish_without_binding(&mut reader) {
            Ok(value) => value,
            Err(value) => {
                *owner = Some(PhaseOwner::BindingFailure(Box::new(value)));
                return Err(TeardownRunRefusal::Binding);
            }
        }
    };
    *owner = Some(PhaseOwner::Finish(Box::new(finish)));
    super::test_cut(TeardownRunPhase::Binding);
    native_current(outside)?;
    *phase = TeardownRunPhase::Retirement;
    outside.remaining()?;
    let Some(PhaseOwner::Finish(finish)) = owner.as_ref() else {
        unreachable!()
    };
    finish
        .check_before_retirement(&operational, &mut reader)
        .map_err(TeardownRunRefusal::Finish)?;
    if !record(outside).native_retired {
        let operation = outside.operation.as_ref().expect("original operation");
        outside
            .ios
            .native
            .as_mut()
            .expect("actual native lock")
            .retire(operation)
            .map_err(|_| recovery())?;
        update(outside, |record| record.native_retired = true)?;
    }
    outside.remaining()?;
    let Some(PhaseOwner::Finish(finish)) = owner.as_ref() else {
        unreachable!()
    };
    finish
        .check_before_retirement(&operational, &mut reader)
        .map_err(TeardownRunRefusal::Finish)?;
    drop(outside.carrier.take());
    let original = Arc::try_unwrap(operational).map_err(|_| TeardownRunRefusal::CarrierShared)?;
    outside.attempts.retirement = true;
    let retired = original.retire_after_settlement();
    let complete = retired.receipt.effect() == ActivityCarrierRetirementEffect::Removed
        && retired.receipt.root_durability() == Durability::Synced
        && retired.receipt.confirmation() == Confirmation::Confirmed
        && retired.receipt.failures().is_empty();
    outside.retirement = Some(retired);
    if !complete {
        return Err(recovery());
    }
    update(outside, |record| record.carrier_retired = true)?;
    *phase = TeardownRunPhase::FinalChecks;
    let Some(PhaseOwner::Finish(finish)) = owner.as_ref() else {
        unreachable!()
    };
    outside.remaining()?;
    finish
        .check_after_retirement(&mut reader)
        .map_err(TeardownRunRefusal::Finish)?;
    outside
        .ios
        .native
        .as_mut()
        .expect("actual lock")
        .confirm_retired(outside.operation.as_ref().expect("original operation"))
        .map_err(|_| recovery())?;
    outside.remaining()?;
    outside
        .ios
        .store
        .take()
        .expect("original record")
        .remove()
        .map_err(|_| recovery())?;
    finish
        .check_after_retirement(&mut reader)
        .map_err(TeardownRunRefusal::Finish)?;
    outside.remaining()
}

fn remote(
    root: &Arc<JournalRoot>,
    outside: &mut Outside,
    owner: &Option<PhaseOwner>,
    reader: &mut TeardownReader<'_>,
    previous: Option<&Record>,
    version: &str,
) -> Result<(), TeardownRunRefusal> {
    use solstone_core_backup_runtime::teardown::owned::IosOperatedPreparation;
    let previous = previous.filter(|r| r.native.is_some());
    let admitted = carrier(owner);
    admitted
        .check(reader)
        .map_err(TeardownRunRefusal::Admission)?;
    let operated = admitted
        .configuration()
        .backup()
        .get("mode")
        .and_then(serde_json::Value::as_str)
        == Some("operated");
    if !operated && admitted.configuration().binding().is_some() {
        return Err(TeardownRunRefusal::UnsupportedDestination);
    }
    let mut preparation: Option<IosOperatedPreparation> = None;
    let destination;
    let mut env = std::collections::BTreeMap::new();
    let mut repository_name = String::new();
    if operated {
        preparation = Some(
            outside
                .operation
                .as_mut()
                .expect("original operation")
                .prepare_ios_operated(admitted, reader, version)
                .map_err(TeardownRunRefusal::Remote)?,
        );
        destination = preparation
            .as_ref()
            .expect("broker result")
            .destination()
            .clone();
    } else {
        let configured = admitted
            .configuration()
            .destination()
            .ok_or(TeardownRunRefusal::UnsupportedDestination)?;
        if admitted.configuration().keys().is_none() {
            return Err(TeardownRunRefusal::UnsupportedDestination);
        }
        let values = solstone_core_backup::assemble_backend_env(configured)
            .map_err(|_| TeardownRunRefusal::UnsupportedDestination)?;
        for (key, value) in values {
            env.insert(
                key,
                Some(
                    value
                        .as_str()
                        .ok_or(TeardownRunRefusal::UnsupportedDestination)?
                        .to_owned(),
                ),
            );
        }
        repository_name = configured.repository.clone();
        let repository::Repository::S3 {
            destination: parsed,
            append_only: false,
            ..
        } = repository::parse(&repository_name, Some(&env))
            .map_err(|_| TeardownRunRefusal::UnsupportedDestination)?
        else {
            return Err(TeardownRunRefusal::UnsupportedDestination);
        };
        destination = parsed;
        outside
            .operation
            .as_mut()
            .expect("original operation")
            .observe_ios_retention(configured)
            .map_err(TeardownRunRefusal::Remote)?;
    }
    let authority = ios_callable::admitted_ios_backup(root.clone())
        .map_err(|_| TeardownRunRefusal::ToolUnavailable)?;
    let context = Arc::new(
        if let Some(previous) = previous {
            if previous
                .native
                .as_ref()
                .expect("recorded native phase")
                .destination
                != destination
            {
                return Err(recovery());
            }
            DestinationContext::reopen(
                authority,
                previous.native.as_ref().expect("recorded native phase"),
            )
        } else {
            DestinationContext::prepare(authority, destination)
        }
        .map_err(|_| recovery())?,
    );
    outside.ios.context = Some(context.clone());
    let mut repository_id = None;
    let mut ids = Vec::new();
    // The only ordinary native reads happen before the final initial custody
    // observation. Pending typed teardown retries never execute Open/List.
    if !operated && previous.is_none() {
        let parsed = repository::parse(&repository_name, Some(&env))
            .map_err(|_| TeardownRunRefusal::UnsupportedDestination)?;
        let keys = admitted.configuration().keys().expect("captured keys");
        let inventory = context
            .inventory(&parsed, &keys.daily_key, operation(outside))
            .map_err(|_| recovery())?;
        repository_id = Some(inventory.0);
        ids = inventory.1;
    }
    let mut lock = if let Some(previous) = previous {
        let destructive = DestructiveAdmission {
            config_sha256: previous.local.original_sha256.clone(),
            repository_id: previous.repository_id.clone().unwrap_or_default(),
            origin: previous.origin.clone(),
            expected: previous
                .native
                .as_ref()
                .expect("recorded native phase")
                .record
                .clone(),
        };
        let scope = if operated {
            None
        } else {
            Some(
                destructive
                    .scope(
                        &previous
                            .native
                            .as_ref()
                            .expect("recorded native phase")
                            .destination,
                        &previous.snapshot_ids,
                    )
                    .map_err(|_| recovery())?,
            )
        };
        let lock = context
            .lock(operation(outside), None, None)
            .or_else(|_| {
                let Some(scope) = scope.as_deref() else {
                    return Err(ios_callable::ExecuteError::Entry(1));
                };
                context.lock(operation(outside), Some(scope), None)
            })
            .map_err(|_| recovery())?;
        let actual = &lock.observation().record;
        if actual
            != &previous
                .native
                .as_ref()
                .expect("recorded native phase")
                .record
            && !(actual.operation == 11
                && actual.scope == scope.clone().unwrap_or_default()
                && actual.origin == previous.origin)
        {
            return Err(recovery());
        }
        lock
    } else {
        context
            .lock(operation(outside), None, None)
            .map_err(|_| recovery())?
    };
    if previous.is_none() {
        let observation = lock.observation().clone();
        let origin = if operated {
            String::new()
        } else {
            observation.record.id.clone()
        };
        if record(outside).local
            != admitted
                .configuration()
                .local_observation()
                .map_err(|_| recovery())?
            || record(outside).carrier
                != admitted
                    .carrier()
                    .recovery_observation()
                    .map_err(|_| recovery())?
        {
            return Err(recovery());
        }
        update(outside, |record| {
            record.native = Some(observation);
            record.origin = origin;
            record.repository_id = repository_id;
            record.snapshot_ids = ids;
        })?;
    }
    admitted
        .check(reader)
        .map_err(TeardownRunRefusal::Admission)?;
    outside.remaining()?;
    if operated {
        outside.ios.native = Some(lock);
        outside
            .operation
            .as_mut()
            .expect("original operation")
            .wipe_ios_operated(
                admitted,
                reader,
                preparation.take().expect("actual broker result"),
            )
            .map_err(TeardownRunRefusal::Remote)?;
        let facts = operation(outside)
            .report()
            .confirmed_facts()
            .ok_or_else(recovery)?;
        update(outside, |record| {
            record.remote_settled = true;
            record.remote_facts = Some(facts);
        })?;
    } else {
        let saved = record(outside).clone();
        let admission = DestructiveAdmission {
            config_sha256: saved.local.original_sha256.clone(),
            repository_id: saved.repository_id.clone().expect("repository id"),
            origin: saved.origin.clone(),
            expected: lock.observation().record.clone(),
        };
        // The worker must take its own ordinary owner exclusion. Release only
        // this observation lock after durable scope/cohort publication.
        drop(lock);
        let parsed = repository::parse(&repository_name, Some(&env))
            .map_err(|_| TeardownRunRefusal::UnsupportedDestination)?;
        let keys = admitted.configuration().keys().expect("captured keys");
        let completed = context
            .prune(
                &parsed,
                &keys.daily_key,
                &saved.snapshot_ids,
                &admission,
                operation(outside),
            )
            .map_err(|_| recovery())?;
        lock = context
            .lock(
                operation(outside),
                Some(&completed.record.scope),
                Some(&completed.record),
            )
            .map_err(|_| recovery())?;
        let actual = lock.observation().clone();
        outside.ios.native = Some(lock);
        outside
            .operation
            .as_mut()
            .expect("original operation")
            .record_ios_prune_completion(saved.snapshot_ids.len() as u32);
        let facts = operation(outside)
            .report()
            .confirmed_facts()
            .ok_or_else(recovery)?;
        update(outside, |record| {
            record.native = Some(actual);
            record.remote_settled = true;
            record.remote_facts = Some(facts);
        })?;
    }
    admitted
        .check(reader)
        .map_err(TeardownRunRefusal::Admission)?;
    outside.remaining()
}

fn reopen_native(
    root: &Arc<JournalRoot>,
    outside: &mut Outside,
    exact: bool,
) -> Result<(), TeardownRunRefusal> {
    let saved = record(outside).clone();
    let authority = ios_callable::admitted_ios_backup(root.clone())
        .map_err(|_| TeardownRunRefusal::ToolUnavailable)?;
    let context = Arc::new(
        DestinationContext::reopen(authority, saved.native.as_ref().ok_or_else(recovery)?)
            .map_err(|_| recovery())?,
    );
    let lock = if saved.native_retired {
        let lock = context
            .lock(operation(outside), None, None)
            .map_err(|_| recovery())?;
        if lock.observation().record.present {
            return Err(recovery());
        }
        lock
    } else {
        let pending = if saved
            .native
            .as_ref()
            .expect("recorded native phase")
            .record
            .operation
            == 11
        {
            Some(
                saved
                    .native
                    .as_ref()
                    .expect("recorded native phase")
                    .record
                    .scope
                    .as_str(),
            )
        } else {
            None
        };
        context
            .lock(
                operation(outside),
                pending,
                if exact {
                    Some(&saved.native.as_ref().expect("recorded native phase").record)
                } else {
                    None
                },
            )
            .map_err(|_| recovery())?
    };
    outside.ios.context = Some(context);
    outside.ios.native = Some(lock);
    outside.remaining()
}

fn finish_already_retired(
    root: &Arc<JournalRoot>,
    outside: &mut Outside,
    reader: &mut TeardownReader<'_>,
    phase: &mut TeardownRunPhase,
) -> Result<(), TeardownRunRefusal> {
    let saved = record(outside).clone();
    if !saved.complete() {
        return Err(recovery());
    }
    let root_dir = MediaDirectory::open(root.clone(), Path::new("")).map_err(|_| recovery())?;
    let absence = || {
        root_dir.revalidate().map_err(|_| recovery())?;
        let entries = root_dir
            .entries_keeping_any_name(16384)
            .map_err(|_| recovery())?
            .ok_or_else(recovery)?;
        if entries
            .iter()
            .any(|e| e.name.to_str() == Some(saved.carrier.directory_name.as_str()))
        {
            return Err(recovery());
        }
        Ok(())
    };
    absence()?;
    reopen_native(root, outside, true)?;
    native_current(outside)?;
    *phase = TeardownRunPhase::Clear;
    outside.remaining()?;
    let configuration = outside
        .ios
        .finished_configuration
        .as_ref()
        .expect("actual local finish capture");
    configuration
        .revalidate()
        .map_err(|_| TeardownRunRefusal::Clear)?;
    reader(root, configuration, None)
        .map_err(|r| TeardownRunRefusal::Admission(TeardownAdmissionRefusal::Reader(r)))?;
    outside.attempts.clear = true;
    let BoundConfigOutcome::Published(publication) = configuration
        .clear_configuration()
        .map_err(|_| TeardownRunRefusal::Clear)?
    else {
        return Err(TeardownRunRefusal::Clear);
    };
    let clear = configuration
        .published_clear_observation(&publication)
        .map_err(|_| TeardownRunRefusal::Clear)?;
    outside.publication = Some(Arc::new(publication));
    update(outside, |record| record.clear = Some(clear))?;
    native_current(outside)?;
    *phase = TeardownRunPhase::FinalChecks;
    absence()?;
    outside.remaining()?;
    let configuration = outside
        .ios
        .finished_configuration
        .as_ref()
        .expect("actual local finish capture");
    configuration
        .revalidate_published_clear(outside.publication.as_ref().expect("actual publication"))
        .map_err(|_| TeardownRunRefusal::Clear)?;
    reader(root, configuration, None)
        .map_err(|r| TeardownRunRefusal::Admission(TeardownAdmissionRefusal::Reader(r)))?;
    outside
        .ios
        .native
        .as_mut()
        .expect("actual lock")
        .confirm_retired(outside.operation.as_ref().expect("original operation"))
        .map_err(|_| recovery())?;
    outside.remaining()?;
    outside
        .ios
        .store
        .take()
        .expect("actual record")
        .remove()
        .map_err(|_| recovery())?;
    absence()?;
    outside.remaining()
}
