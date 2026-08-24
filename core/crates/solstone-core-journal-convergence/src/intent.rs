// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::os::fd::OwnedFd;

use solstone_core_journal_io::create_directory_bound;

use crate::digest::digest_value;
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::init::StoreDirs;
use crate::layout::{DayKey, INTENTS, intent_name};
use crate::projection::{marker_present, verify_projection_binding};
use crate::schema::{
    Active, Adoption, Intent, OPERATION_ADVANCE_DIRTY, Predecessor, PresentAbsent,
    ProjectionBinding, ROLE_INTENT, ROLE_VIRGIN, SCHEMA_VERSION, VirginProof, day_set_subdigest,
    intent_digest, read_json, write_json_exclusive,
};
use crate::store::ConvergenceStore;
use crate::walk::open_dir;

pub(crate) fn open_intents_dir(dirs: &StoreDirs) -> Result<Option<OwnedFd>, ConvergenceError> {
    open_dir(&dirs.convergence, INTENTS)
}

pub(crate) fn ensure_intents_dir(dirs: &StoreDirs) -> Result<OwnedFd, ConvergenceError> {
    create_directory_bound(&dirs.convergence, std::ffi::OsStr::new(INTENTS), 0o700).map_err(
        |error| ConvergenceError::Io {
            operation: "create intents directory",
            role: DurableRole::Directory,
            source: std::io::Error::other(error.to_string()),
        },
    )?;
    open_dir(&dirs.convergence, INTENTS)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })
}

pub(crate) fn read_intent(
    dirs: &StoreDirs,
    serial: u64,
) -> Result<Option<Intent>, ConvergenceError> {
    let Some(intents) = open_intents_dir(dirs)? else {
        return Ok(None);
    };
    read_json(&intents, &intent_name(serial), DurableRole::Intent)
}

pub(crate) fn write_intent(dirs: &StoreDirs, intent: &Intent) -> Result<(), ConvergenceError> {
    let intents = ensure_intents_dir(dirs)?;
    match write_json_exclusive(
        &intents,
        &intent_name(intent.serial),
        intent,
        DurableRole::Intent,
    ) {
        Ok(_) => Ok(()),
        Err(ConvergenceError::PreservedPrior { .. }) => {
            let existing =
                read_json::<Intent>(&intents, &intent_name(intent.serial), DurableRole::Intent)?
                    .ok_or(ConvergenceError::Unknown {
                        role: DurableRole::Intent,
                    })?;
            if existing.intent_digest != intent.intent_digest {
                return Err(ConvergenceError::Refused(Refusal::IntentMismatch));
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn virgin_digest(
    store: &ConvergenceStore,
    adoption: &Adoption,
    day: &DayKey,
) -> Result<String, ConvergenceError> {
    let proof = VirginProof {
        role: ROLE_VIRGIN.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        adoption_id: adoption.adoption_id.clone(),
        day: day.as_str().to_owned(),
    };
    Ok(digest_value(&proof)?.as_hex().to_owned())
}

pub(crate) fn day_is_store_genesis(
    dirs: &StoreDirs,
    day: &DayKey,
) -> Result<bool, ConvergenceError> {
    let ever = crate::walk::open_file(&dirs.days, &format!("{}.ever.wit.json", day.as_str()))?;
    let head = crate::walk::open_file(&dirs.days, &format!("{}.head.json", day.as_str()))?;
    let record_dir = crate::walk::open_dir(&dirs.records, day.as_str())?;
    Ok(ever.is_none() && head.is_none() && record_dir.is_none())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_virgin_intent(
    store: &ConvergenceStore,
    days: &[DayKey],
    serial: u64,
    owner_digest: &str,
    claim_revision: u64,
    prior_claim_head_revision: u64,
    prior_claim_head_digest: &str,
    adoptions: &BTreeMap<String, Adoption>,
) -> Result<Intent, ConvergenceError> {
    let mut prior_day_revisions = BTreeMap::new();
    let mut proposed_day_revisions = BTreeMap::new();
    let mut proposed_dirty_generations = BTreeMap::new();
    let mut predecessors = BTreeMap::new();
    let mut projections = BTreeMap::new();
    for day in days {
        let adoption = adoptions
            .get(day.as_str())
            .ok_or(ConvergenceError::Unknown {
                role: DurableRole::Adoption,
            })?;
        prior_day_revisions.insert(day.as_str().to_owned(), 0);
        proposed_day_revisions.insert(day.as_str().to_owned(), 1);
        proposed_dirty_generations.insert(day.as_str().to_owned(), 1);
        predecessors.insert(
            day.as_str().to_owned(),
            Predecessor::Virgin {
                digest: virgin_digest(store, adoption, day)?,
            },
        );
        let binding = ProjectionBinding {
            prior_stream: PresentAbsent::Absent,
            prior_daily: PresentAbsent::Absent,
            proposed_stream: marker_present(store, adoption, day, 1, serial)?,
            proposed_daily: PresentAbsent::Absent,
        };
        verify_projection_binding(&binding)?;
        projections.insert(day.as_str().to_owned(), binding);
    }
    let mut intent = Intent {
        role: ROLE_INTENT.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        serial,
        operation: OPERATION_ADVANCE_DIRTY.to_owned(),
        day_set: days.iter().map(|day| day.as_str().to_owned()).collect(),
        day_set_subdigest: day_set_subdigest(days)?.as_hex().to_owned(),
        owner_binding_digest: owner_digest.to_owned(),
        claim_revision,
        prior_claim_head_revision,
        prior_claim_head_digest: prior_claim_head_digest.to_owned(),
        prior_day_revisions,
        proposed_day_revisions,
        proposed_dirty_generations,
        predecessors,
        projections,
        intent_digest: String::new(),
    };
    intent.intent_digest = intent_digest(&intent)?.as_hex().to_owned();
    Ok(intent)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_allocation_intent(
    store: &ConvergenceStore,
    days: &[DayKey],
    serial: u64,
    owner_digest: &str,
    claim_revision: u64,
    prior_claim_head_revision: u64,
    prior_claim_head_digest: &str,
    adoptions: &BTreeMap<String, Adoption>,
    classes: &BTreeMap<String, crate::clearance::PredecessorClass>,
    snapshots: &BTreeMap<String, crate::store::DaySnapshot>,
) -> Result<Intent, ConvergenceError> {
    let mut prior_day_revisions = BTreeMap::new();
    let mut proposed_day_revisions = BTreeMap::new();
    let mut proposed_dirty_generations = BTreeMap::new();
    let mut predecessors = BTreeMap::new();
    let mut projections = BTreeMap::new();
    for day in days {
        let adoption = adoptions
            .get(day.as_str())
            .ok_or(ConvergenceError::Unknown {
                role: DurableRole::Adoption,
            })?;
        let class = classes.get(day.as_str()).ok_or(ConvergenceError::Unknown {
            role: DurableRole::ClearanceMember,
        })?;
        match class {
            crate::clearance::PredecessorClass::Virgin { digest } => {
                prior_day_revisions.insert(day.as_str().to_owned(), 0);
                proposed_day_revisions.insert(day.as_str().to_owned(), 1);
                proposed_dirty_generations.insert(day.as_str().to_owned(), 1);
                predecessors.insert(
                    day.as_str().to_owned(),
                    Predecessor::Virgin {
                        digest: digest.clone(),
                    },
                );
                let binding = ProjectionBinding {
                    prior_stream: PresentAbsent::Absent,
                    prior_daily: PresentAbsent::Absent,
                    proposed_stream: marker_present(store, adoption, day, 1, serial)?,
                    proposed_daily: PresentAbsent::Absent,
                };
                verify_projection_binding(&binding)?;
                projections.insert(day.as_str().to_owned(), binding);
            }
            crate::clearance::PredecessorClass::Member {
                member_digest,
                barrier_digest,
            } => {
                let snapshot = snapshots
                    .get(day.as_str())
                    .ok_or(ConvergenceError::Unknown {
                        role: DurableRole::Record,
                    })?;
                prior_day_revisions.insert(day.as_str().to_owned(), snapshot.record_revision);
                proposed_day_revisions
                    .insert(day.as_str().to_owned(), snapshot.record_revision + 1);
                proposed_dirty_generations
                    .insert(day.as_str().to_owned(), snapshot.dirty_generation + 1);
                predecessors.insert(
                    day.as_str().to_owned(),
                    Predecessor::Member {
                        member_digest: member_digest.clone(),
                        barrier_digest: barrier_digest.clone(),
                    },
                );
                let binding = ProjectionBinding {
                    prior_stream: marker_present(
                        store,
                        adoption,
                        day,
                        snapshot.dirty_generation,
                        snapshot.dirty_by_transition_serial,
                    )?,
                    prior_daily: PresentAbsent::Absent,
                    proposed_stream: marker_present(
                        store,
                        adoption,
                        day,
                        snapshot.dirty_generation + 1,
                        serial,
                    )?,
                    proposed_daily: PresentAbsent::Absent,
                };
                verify_projection_binding(&binding)?;
                projections.insert(day.as_str().to_owned(), binding);
            }
        }
    }
    let mut intent = Intent {
        role: ROLE_INTENT.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        serial,
        operation: OPERATION_ADVANCE_DIRTY.to_owned(),
        day_set: days.iter().map(|day| day.as_str().to_owned()).collect(),
        day_set_subdigest: day_set_subdigest(days)?.as_hex().to_owned(),
        owner_binding_digest: owner_digest.to_owned(),
        claim_revision,
        prior_claim_head_revision,
        prior_claim_head_digest: prior_claim_head_digest.to_owned(),
        prior_day_revisions,
        proposed_day_revisions,
        proposed_dirty_generations,
        predecessors,
        projections,
        intent_digest: String::new(),
    };
    intent.intent_digest = intent_digest(&intent)?.as_hex().to_owned();
    Ok(intent)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_later_intent(
    store: &ConvergenceStore,
    days: &[DayKey],
    serial: u64,
    owner_digest: &str,
    claim_revision: u64,
    prior_claim_head_revision: u64,
    prior_claim_head_digest: &str,
    adoptions: &BTreeMap<String, Adoption>,
    snapshots: &BTreeMap<String, crate::store::DaySnapshot>,
    prior_intent: &Intent,
) -> Result<Intent, ConvergenceError> {
    let mut prior_day_revisions = BTreeMap::new();
    let mut proposed_day_revisions = BTreeMap::new();
    let mut proposed_dirty_generations = BTreeMap::new();
    let mut predecessors = BTreeMap::new();
    let mut projections = BTreeMap::new();
    for day in days {
        let adoption = adoptions
            .get(day.as_str())
            .ok_or(ConvergenceError::Unknown {
                role: DurableRole::Adoption,
            })?;
        let snapshot = snapshots
            .get(day.as_str())
            .ok_or(ConvergenceError::Unknown {
                role: DurableRole::Record,
            })?;
        prior_day_revisions.insert(day.as_str().to_owned(), snapshot.record_revision);
        proposed_day_revisions.insert(day.as_str().to_owned(), snapshot.record_revision + 1);
        proposed_dirty_generations.insert(day.as_str().to_owned(), snapshot.dirty_generation + 1);
        predecessors.insert(
            day.as_str().to_owned(),
            prior_intent
                .predecessors
                .get(day.as_str())
                .cloned()
                .ok_or(ConvergenceError::Refused(Refusal::ChangedPredecessor))?,
        );
        let binding = ProjectionBinding {
            prior_stream: marker_present(
                store,
                adoption,
                day,
                snapshot.dirty_generation,
                snapshot.dirty_by_transition_serial,
            )?,
            prior_daily: PresentAbsent::Absent,
            proposed_stream: marker_present(
                store,
                adoption,
                day,
                snapshot.dirty_generation + 1,
                serial,
            )?,
            proposed_daily: PresentAbsent::Absent,
        };
        verify_projection_binding(&binding)?;
        projections.insert(day.as_str().to_owned(), binding);
    }
    let mut intent = Intent {
        role: ROLE_INTENT.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        serial,
        operation: OPERATION_ADVANCE_DIRTY.to_owned(),
        day_set: days.iter().map(|day| day.as_str().to_owned()).collect(),
        day_set_subdigest: day_set_subdigest(days)?.as_hex().to_owned(),
        owner_binding_digest: owner_digest.to_owned(),
        claim_revision,
        prior_claim_head_revision,
        prior_claim_head_digest: prior_claim_head_digest.to_owned(),
        prior_day_revisions,
        proposed_day_revisions,
        proposed_dirty_generations,
        predecessors,
        projections,
        intent_digest: String::new(),
    };
    intent.intent_digest = intent_digest(&intent)?.as_hex().to_owned();
    Ok(intent)
}

pub(crate) fn verify_intent_matches_claim(
    intent: &Intent,
    expected_digest: &str,
) -> Result<(), ConvergenceError> {
    let computed = intent_digest(intent)?;
    if computed.as_hex() != intent.intent_digest {
        return Err(ConvergenceError::Refused(Refusal::IntentDigestMismatch));
    }
    if intent.intent_digest != expected_digest {
        return Err(ConvergenceError::Refused(Refusal::IntentMismatch));
    }
    Ok(())
}

pub(crate) fn write_active(dirs: &StoreDirs, active: &Active) -> Result<(), ConvergenceError> {
    create_directory_bound(
        &dirs.convergence,
        std::ffi::OsStr::new(crate::layout::ACTIVES),
        0o700,
    )
    .map_err(|error| ConvergenceError::Io {
        operation: "create actives directory",
        role: DurableRole::Directory,
        source: std::io::Error::other(error.to_string()),
    })?;
    let actives =
        open_dir(&dirs.convergence, crate::layout::ACTIVES)?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::Directory,
        })?;
    match write_json_exclusive(
        &actives,
        &crate::layout::active_name(active.serial),
        active,
        DurableRole::Active,
    ) {
        Ok(_) => Ok(()),
        Err(ConvergenceError::PreservedPrior { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

#[allow(dead_code)]
pub(crate) fn read_active(
    dirs: &StoreDirs,
    serial: u64,
) -> Result<Option<Active>, ConvergenceError> {
    let Some(actives) = open_dir(&dirs.convergence, crate::layout::ACTIVES)? else {
        return Ok(None);
    };
    read_json(
        &actives,
        &crate::layout::active_name(serial),
        DurableRole::Active,
    )
}
