// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exclusive create, lease, and no-replace publish for one oplog leaf.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::io::{self, ErrorKind, Write};
#[cfg(any(test, feature = "test-hooks"))]
use std::time::Duration;

use chrono::{DateTime, FixedOffset};

use super::admission::encode_oplog_admission;
use super::sample_local_instant;

#[cfg(any(test, feature = "test-hooks"))]
use super::lock::acquire_oplog_namespace_lock_with_test_timing;
use super::lock::{OplogNamespaceLockError, acquire_oplog_namespace_lock};
use super::name::{
    OplogFormat, derive_day_key_and_opened_field, file_id_hex, format_oplog_name,
    oplog_name_from_parts, original_is_admissible,
};
use super::namespace::{OplogDayHealth, OplogNamespaceError, admit_day_health_directory};
use super::reason::{
    NamedOccupant, NamedOpen, OplogAdmissionCause, OplogCollisionOccupant, OplogCollisionRecord,
    OplogCreateError, OplogCreateEvidence, OplogCreateReason, OplogEvidenceCheckpoint,
    OplogFileIdentity, OplogGapCause, OplogIdentityObservation, OplogPublishReason,
    OplogStageCause, OplogVerifiedAt, StageError,
};
use super::writer::OplogWriter;
use crate::journal_root::JournalRoot;
use crate::lease::LeaseProbe;

#[cfg(unix)]
use super::unix as platform;
#[cfg(windows)]
use super::windows as platform;

/// Maximum dest-occupied retries, each consuming one pre-drawn file id.
pub const OPLOG_CREATE_ATTEMPTS: usize = 8;
/// Maximum `draw_file_id` calls used to collect [`OPLOG_CREATE_ATTEMPTS`] distinct ids.
pub const OPLOG_FILE_ID_DRAW_BUDGET: usize = 64;

enum LockTiming {
    Default,
    #[cfg(any(test, feature = "test-hooks"))]
    Explicit(Duration, Duration),
}

fn bare(reason: OplogCreateReason) -> OplogCreateError {
    OplogCreateEvidence::not_established().fail(reason)
}

/// Create one exclusive append-only operational log under `root`.
pub fn create_oplog(
    root: JournalRoot,
    source_original: &str,
    run_original: &str,
    format: OplogFormat,
) -> Result<OplogWriter, OplogCreateError> {
    if !original_is_admissible(source_original) || !original_is_admissible(run_original) {
        return Err(bare(OplogCreateReason::InvalidField));
    }
    let instant = sample_local_instant()?;
    create_with_timing(
        root,
        source_original,
        run_original,
        format,
        instant,
        LockTiming::Default,
    )
}

/// Create with caller-supplied namespace-lock timing.
#[cfg(any(test, feature = "test-hooks"))]
pub fn create_oplog_with_test_timing(
    root: JournalRoot,
    source_original: &str,
    run_original: &str,
    format: OplogFormat,
    instant: DateTime<FixedOffset>,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<OplogWriter, OplogCreateError> {
    create_with_timing(
        root,
        source_original,
        run_original,
        format,
        instant,
        LockTiming::Explicit(timeout, poll_interval),
    )
}

fn create_with_timing(
    root: JournalRoot,
    source_original: &str,
    run_original: &str,
    format: OplogFormat,
    instant: DateTime<FixedOffset>,
    timing: LockTiming,
) -> Result<OplogWriter, OplogCreateError> {
    if !original_is_admissible(source_original) || !original_is_admissible(run_original) {
        return Err(bare(OplogCreateReason::InvalidField));
    }
    let (day, opened) = derive_day_key_and_opened_field(instant);
    let ids = draw_distinct_file_ids()?;
    let names = ids.map(|file_id_bytes| {
        oplog_name_from_parts(
            source_original,
            run_original,
            opened.clone(),
            file_id_hex(&file_id_bytes),
            format,
        )
    });
    let header = encode_oplog_admission(&names);
    let health = match admit_day_health_directory(root, &day) {
        Ok(health) => health,
        Err(error) => return Err(bare(map_namespace_error(error))),
    };
    let mut evidence = OplogCreateEvidence::established(health.identity());
    let _lock = match acquire_lock(&health, timing) {
        Ok(lock) => lock,
        Err(reason) => return Err(evidence.fail(reason)),
    };

    if let Err(reason) = checkpoint(OplogCreatePrimitive::Stage) {
        return Err(evidence.fail(reason));
    }
    let first_dest = format_oplog_name(&names[0]);
    let mut staged = match platform::stage_exclusive(&health, OsStr::new(&first_dest)) {
        Ok(staged) => staged,
        Err(StageError::Allocate) => {
            return Err(evidence.fail(OplogCreateReason::Stage(OplogStageCause::Allocate)));
        }
        Err(StageError::Leftover {
            name,
            cause,
            identity,
        }) => {
            if let Some(identity) = identity {
                observe_stage_leaf(
                    &health,
                    &name,
                    identity,
                    OplogEvidenceCheckpoint::Stage,
                    &mut evidence,
                );
            } else {
                evidence.gap(OplogEvidenceCheckpoint::Stage, OplogGapCause::Io);
            }
            return Err(evidence.fail(OplogCreateReason::Stage(cause.stage_cause())));
        }
    };
    let original_stage_name = staged.stage_name.clone();
    let original_identity = staged.identity;
    let mut witnesses = Vec::with_capacity(OPLOG_CREATE_ATTEMPTS);
    let mut attempted: Vec<(u8, OsString)> = Vec::new();

    if let Err(reason) = checkpoint(OplogCreatePrimitive::Admission) {
        return Err(fail_after_stage(
            reason,
            evidence,
            &health,
            &staged,
            original_identity,
            &attempted,
            &witnesses,
        ));
    }
    if let Err(cause) = write_admission(&mut staged.file, &header) {
        return Err(fail_after_stage(
            OplogCreateReason::Admission(cause),
            evidence,
            &health,
            &staged,
            original_identity,
            &attempted,
            &witnesses,
        ));
    }
    barrier(OplogCreatePrimitive::AfterStageBeforeLease);
    if let Err(reason) = checkpoint(OplogCreatePrimitive::Lease) {
        return Err(fail_after_stage(
            reason,
            evidence,
            &health,
            &staged,
            original_identity,
            &attempted,
            &witnesses,
        ));
    }
    record_event(OplogCreateEvent::Lease);
    let lease = match platform::lease_staged(&staged.file) {
        Ok(Some(lease)) => lease,
        Ok(None) => {
            return Err(fail_after_stage(
                OplogCreateReason::LeaseFailed,
                evidence,
                &health,
                &staged,
                original_identity,
                &attempted,
                &witnesses,
            ));
        }
        Err(_) => {
            return Err(fail_after_stage(
                OplogCreateReason::LeaseIo,
                evidence,
                &health,
                &staged,
                original_identity,
                &attempted,
                &witnesses,
            ));
        }
    };
    barrier(OplogCreatePrimitive::AfterLeaseBeforePublish);

    for (index, name) in names.iter().enumerate() {
        let ordinal = (index + 1) as u8;
        let dest = format_oplog_name(name);
        let dest_os = OsStr::new(&dest);
        if let Err(component) = health.revalidate_publication_ancestors() {
            observe_stage_leaf(
                &health,
                staged.stage_name.as_os_str(),
                original_identity,
                OplogEvidenceCheckpoint::Stage,
                &mut evidence,
            );
            evidence.gap(
                OplogEvidenceCheckpoint::AncestorRevalidation { ordinal, component },
                OplogGapCause::Changed,
            );
            return Err(classify_failure(
                OplogCreateReason::Publish(OplogPublishReason::AncestorRevalidation),
                evidence,
                &health,
                &staged,
                original_identity,
                &attempted,
                &witnesses,
            ));
        }
        attempted.push((ordinal, OsString::from(&dest)));
        if let Err(reason) = checkpoint(OplogCreatePrimitive::Rename) {
            evidence.gap(
                OplogEvidenceCheckpoint::Rename { ordinal },
                OplogGapCause::Io,
            );
            return Err(fail_after_stage(
                reason,
                evidence,
                &health,
                &staged,
                original_identity,
                &attempted,
                &witnesses,
            ));
        }
        let outcome = match platform::rename_stage(&health, &staged, dest_os) {
            Ok(()) => RenameOutcome::Landed,
            Err(error) => classify_rename(&error),
        };
        match outcome {
            RenameOutcome::Landed => {
                if let Err(reason) = checkpoint(OplogCreatePrimitive::AfterRename) {
                    return Err(fail_after_stage(
                        reason,
                        evidence,
                        &health,
                        &staged,
                        original_identity,
                        &attempted,
                        &witnesses,
                    ));
                }
                return finish_landed(
                    health,
                    staged,
                    lease,
                    dest,
                    original_identity,
                    evidence,
                    &attempted,
                    &witnesses,
                );
            }
            RenameOutcome::Occupied => match handle_occupied(
                &health,
                &staged,
                dest_os,
                dest.clone(),
                ordinal,
                original_identity,
                original_stage_name.as_os_str(),
                &mut evidence,
                &mut witnesses,
            ) {
                OccupiedAction::Continue => continue,
                OccupiedAction::Landed => {
                    return finish_landed(
                        health,
                        staged,
                        lease,
                        dest,
                        original_identity,
                        evidence,
                        &attempted,
                        &witnesses,
                    );
                }
                OccupiedAction::Fail(reason) => {
                    return Err(fail_after_stage(
                        reason,
                        evidence,
                        &health,
                        &staged,
                        original_identity,
                        &attempted,
                        &witnesses,
                    ));
                }
            },
            RenameOutcome::Unsupported => {
                return Err(classify_failure(
                    OplogCreateReason::Publish(OplogPublishReason::Rename),
                    evidence,
                    &health,
                    &staged,
                    original_identity,
                    &attempted,
                    &witnesses,
                ));
            }
            RenameOutcome::SourceAbsent | RenameOutcome::Ambiguous => {
                match reconcile_rename(
                    &health,
                    &staged,
                    dest_os,
                    dest.clone(),
                    ordinal,
                    original_identity,
                    original_stage_name.as_os_str(),
                    &mut evidence,
                    &mut witnesses,
                ) {
                    OccupiedAction::Continue => continue,
                    OccupiedAction::Landed => {
                        return finish_landed(
                            health,
                            staged,
                            lease,
                            dest,
                            original_identity,
                            evidence,
                            &attempted,
                            &witnesses,
                        );
                    }
                    OccupiedAction::Fail(reason) => {
                        return Err(fail_after_stage(
                            reason,
                            evidence,
                            &health,
                            &staged,
                            original_identity,
                            &attempted,
                            &witnesses,
                        ));
                    }
                }
            }
        }
    }

    Err(classify_failure(
        OplogCreateReason::Publish(OplogPublishReason::DestinationExhaustion),
        evidence,
        &health,
        &staged,
        original_identity,
        &attempted,
        &witnesses,
    ))
}

#[derive(Clone, Copy)]
enum RenameOutcome {
    Landed,
    Occupied,
    Unsupported,
    SourceAbsent,
    Ambiguous,
}

fn classify_rename(error: &io::Error) -> RenameOutcome {
    #[cfg(unix)]
    {
        match crate::claim_remove::classify_rename_error(error) {
            crate::claim_remove::RenameErrorClass::Occupied => RenameOutcome::Occupied,
            crate::claim_remove::RenameErrorClass::Unsupported => RenameOutcome::Unsupported,
            crate::claim_remove::RenameErrorClass::SourceAbsent => RenameOutcome::SourceAbsent,
            crate::claim_remove::RenameErrorClass::Ambiguous => RenameOutcome::Ambiguous,
        }
    }
    #[cfg(windows)]
    {
        match platform::classify_windows_rename_error(error) {
            platform::OplogRenameClass::Occupied => RenameOutcome::Occupied,
            platform::OplogRenameClass::Unsupported => RenameOutcome::Unsupported,
            platform::OplogRenameClass::SourceAbsent => RenameOutcome::SourceAbsent,
            platform::OplogRenameClass::Ambiguous => RenameOutcome::Ambiguous,
        }
    }
}

enum OccupiedAction {
    Continue,
    Landed,
    Fail(OplogCreateReason),
}

struct CollisionWitness {
    ordinal: u8,
    identity: OplogFileIdentity,
    file: std::fs::File,
}

#[allow(clippy::too_many_arguments)]
fn handle_occupied(
    health: &OplogDayHealth,
    staged: &platform::StagedFile,
    dest: &OsStr,
    dest_owned: String,
    ordinal: u8,
    original_identity: OplogFileIdentity,
    original_stage_name: &OsStr,
    evidence: &mut OplogCreateEvidence,
    witnesses: &mut Vec<CollisionWitness>,
) -> OccupiedAction {
    assert_eq!(staged.stage_name.as_os_str(), original_stage_name);
    assert_eq!(staged.identity, original_identity);
    assert_eq!(
        platform::identity_of(&staged.file).ok(),
        Some(original_identity)
    );
    match platform::open_named(health, dest) {
        Ok(NamedOpen::Regular {
            file,
            identity,
            nlink: _,
        }) if identity != original_identity => {
            let verified_at = OplogVerifiedAt::new(
                Some(OsString::from(&dest_owned)),
                OplogEvidenceCheckpoint::AfterForeignCollision { ordinal },
            );
            evidence.collision(OplogCollisionRecord::new(
                ordinal,
                OsString::from(&dest_owned),
                OplogCollisionOccupant::Foreign {
                    identity,
                    verified_at: verified_at.clone(),
                },
            ));
            evidence.observe(OplogIdentityObservation::foreign_landed(verified_at));
            debug_assert!(witnesses.len() < OPLOG_CREATE_ATTEMPTS);
            witnesses.push(CollisionWitness {
                ordinal,
                identity,
                file,
            });
            barrier(OplogCreatePrimitive::AfterForeignCollision);
            if let Err(reason) = checkpoint(OplogCreatePrimitive::AfterForeignCollision) {
                return OccupiedAction::Fail(reason);
            }
            OccupiedAction::Continue
        }
        Ok(NamedOpen::Regular { identity, .. }) if identity == original_identity => {
            // Occupied while dest names our inode: the .tmp source still exists,
            // so the dest is another link to the same inode. That is Alias.
            evidence.observe(OplogIdentityObservation::own_landed(OplogVerifiedAt::new(
                Some(OsString::from(&dest_owned)),
                OplogEvidenceCheckpoint::DestinationInspection { ordinal },
            )));
            OccupiedAction::Fail(OplogCreateReason::Publish(OplogPublishReason::Alias))
        }
        Ok(NamedOpen::Absent) => OccupiedAction::Fail(OplogCreateReason::Publish(
            OplogPublishReason::Reconciliation,
        )),
        Ok(NamedOpen::Other) => {
            evidence.gap(
                OplogEvidenceCheckpoint::DestinationInspection { ordinal },
                OplogGapCause::Changed,
            );
            OccupiedAction::Fail(OplogCreateReason::Publish(
                OplogPublishReason::DestinationInspection,
            ))
        }
        Err(_) => {
            evidence.gap(
                OplogEvidenceCheckpoint::DestinationInspection { ordinal },
                OplogGapCause::Io,
            );
            OccupiedAction::Fail(OplogCreateReason::Publish(
                OplogPublishReason::DestinationInspection,
            ))
        }
        Ok(NamedOpen::Regular { .. }) => OccupiedAction::Fail(OplogCreateReason::Publish(
            OplogPublishReason::DestinationInspection,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn reconcile_rename(
    health: &OplogDayHealth,
    staged: &platform::StagedFile,
    dest: &OsStr,
    dest_owned: String,
    ordinal: u8,
    original_identity: OplogFileIdentity,
    original_stage_name: &OsStr,
    evidence: &mut OplogCreateEvidence,
    witnesses: &mut Vec<CollisionWitness>,
) -> OccupiedAction {
    let stage_state = platform::inspect_named(health, original_stage_name);
    let dest_state = platform::inspect_named(health, dest);
    match (stage_state, dest_state) {
        (Err(_), _) | (_, Err(_)) => {
            evidence.gap(
                OplogEvidenceCheckpoint::DestinationInspection { ordinal },
                OplogGapCause::Io,
            );
            OccupiedAction::Fail(OplogCreateReason::Publish(
                OplogPublishReason::Reconciliation,
            ))
        }
        (Ok(NamedOccupant::Absent), Ok(NamedOccupant::Regular { identity, .. }))
            if identity == original_identity =>
        {
            OccupiedAction::Landed
        }
        (
            Ok(NamedOccupant::Regular {
                identity: stage_id, ..
            }),
            Ok(NamedOccupant::Regular {
                identity: dest_id, ..
            }),
        ) if stage_id == original_identity && dest_id != original_identity => handle_occupied(
            health,
            staged,
            dest,
            dest_owned,
            ordinal,
            original_identity,
            original_stage_name,
            evidence,
            witnesses,
        ),
        (Ok(NamedOccupant::Regular { identity, .. }), Ok(NamedOccupant::Absent))
            if identity == original_identity =>
        {
            OccupiedAction::Fail(OplogCreateReason::Publish(
                OplogPublishReason::Reconciliation,
            ))
        }
        (
            Ok(NamedOccupant::Regular {
                identity: stage_id, ..
            }),
            Ok(NamedOccupant::Regular {
                identity: dest_id, ..
            }),
        ) if stage_id == original_identity && dest_id == original_identity => {
            evidence.gap(
                OplogEvidenceCheckpoint::DestinationInspection { ordinal },
                OplogGapCause::Inconsistent,
            );
            OccupiedAction::Fail(OplogCreateReason::Publish(
                OplogPublishReason::Reconciliation,
            ))
        }
        _ => OccupiedAction::Fail(OplogCreateReason::Publish(
            OplogPublishReason::Reconciliation,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_landed(
    health: OplogDayHealth,
    staged: platform::StagedFile,
    lease: crate::lease::SelfLease,
    dest: String,
    original_identity: OplogFileIdentity,
    mut evidence: OplogCreateEvidence,
    attempted: &[(u8, OsString)],
    witnesses: &[CollisionWitness],
) -> Result<OplogWriter, OplogCreateError> {
    if let Err(reason) = checkpoint(OplogCreatePrimitive::AfterRenameBeforeDirectorySync) {
        return Err(fail_after_stage(
            reason,
            evidence,
            &health,
            &staged,
            original_identity,
            attempted,
            witnesses,
        ));
    }
    if force_dest_identity_io() {
        evidence.gap(OplogEvidenceCheckpoint::AfterRename, OplogGapCause::Io);
        return Err(fail_after_stage(
            OplogCreateReason::Publish(OplogPublishReason::DestinationInspection),
            evidence,
            &health,
            &staged,
            original_identity,
            attempted,
            witnesses,
        ));
    }
    match platform::inspect_named(&health, OsStr::new(&dest)) {
        Ok(NamedOccupant::Regular { identity, nlink }) if identity == original_identity => {
            evidence.observe(OplogIdentityObservation::own_landed(OplogVerifiedAt::new(
                Some(OsString::from(&dest)),
                OplogEvidenceCheckpoint::AfterRename,
            )));
            if nlink != 1 {
                return Err(fail_after_stage(
                    OplogCreateReason::Publish(OplogPublishReason::Alias),
                    evidence,
                    &health,
                    &staged,
                    original_identity,
                    attempted,
                    witnesses,
                ));
            }
        }
        Ok(_) => {
            return Err(fail_after_stage(
                OplogCreateReason::Publish(OplogPublishReason::DestinationInspection),
                evidence,
                &health,
                &staged,
                original_identity,
                attempted,
                witnesses,
            ));
        }
        Err(_) => {
            evidence.gap(OplogEvidenceCheckpoint::AfterRename, OplogGapCause::Io);
            return Err(fail_after_stage(
                OplogCreateReason::Publish(OplogPublishReason::DestinationInspection),
                evidence,
                &health,
                &staged,
                original_identity,
                attempted,
                witnesses,
            ));
        }
    }
    record_event(OplogCreateEvent::Publish);
    if sync_published(&health).is_err() {
        evidence.gap(OplogEvidenceCheckpoint::DirectorySync, OplogGapCause::Io);
        return Err(fail_after_stage(
            OplogCreateReason::Publish(OplogPublishReason::DirectorySync),
            evidence,
            &health,
            &staged,
            original_identity,
            attempted,
            witnesses,
        ));
    }
    barrier(OplogCreatePrimitive::AfterRenameBeforeDirectorySync);
    if let Err(reason) = final_binding_gate(
        &health,
        &staged,
        OsStr::new(&dest),
        original_identity,
        &mut evidence,
    ) {
        return Err(fail_after_stage(
            reason,
            evidence,
            &health,
            &staged,
            original_identity,
            attempted,
            witnesses,
        ));
    }
    Ok(OplogWriter::new(staged.file, lease, dest))
}

fn final_binding_gate(
    health: &OplogDayHealth,
    staged: &platform::StagedFile,
    dest: &OsStr,
    original_identity: OplogFileIdentity,
    evidence: &mut OplogCreateEvidence,
) -> Result<(), OplogCreateReason> {
    if health.revalidate_publication_ancestors().is_err() {
        evidence.gap(
            OplogEvidenceCheckpoint::FinalBinding,
            OplogGapCause::Changed,
        );
        return Err(OplogCreateReason::Publish(OplogPublishReason::FinalBinding));
    }
    match platform::inspect_named(health, dest) {
        Ok(NamedOccupant::Regular { identity, .. }) if identity == original_identity => {}
        Ok(_) => {
            return Err(OplogCreateReason::Publish(OplogPublishReason::FinalBinding));
        }
        Err(_) => {
            evidence.gap(OplogEvidenceCheckpoint::FinalBinding, OplogGapCause::Io);
            return Err(OplogCreateReason::Publish(OplogPublishReason::FinalBinding));
        }
    }
    match platform::inspect_named(health, staged.stage_name.as_os_str()) {
        Ok(NamedOccupant::Absent) => {}
        Ok(NamedOccupant::Regular { identity, .. }) if identity != original_identity => {
            evidence.observe(OplogIdentityObservation::foreign_noncanonical(
                OplogVerifiedAt::new(
                    Some(staged.stage_name.clone()),
                    OplogEvidenceCheckpoint::FinalBinding,
                ),
            ));
            return Err(OplogCreateReason::Publish(OplogPublishReason::FinalBinding));
        }
        Ok(_) => {
            return Err(OplogCreateReason::Publish(OplogPublishReason::FinalBinding));
        }
        Err(_) => {
            evidence.gap(OplogEvidenceCheckpoint::FinalBinding, OplogGapCause::Io);
            return Err(OplogCreateReason::Publish(OplogPublishReason::FinalBinding));
        }
    }
    match platform::nlink_of(&staged.file) {
        Ok(1) => Ok(()),
        Ok(nlink) => {
            evidence.observe(OplogIdentityObservation::own_multiple_links(
                nlink,
                OplogVerifiedAt::new(None, OplogEvidenceCheckpoint::FinalBinding),
            ));
            Err(OplogCreateReason::Publish(OplogPublishReason::FinalBinding))
        }
        Err(_) => {
            evidence.gap(
                OplogEvidenceCheckpoint::RetainedHandle,
                OplogGapCause::UnobservableHandle,
            );
            Err(OplogCreateReason::Publish(OplogPublishReason::FinalBinding))
        }
    }
}

fn final_binding_evidence_only(
    health: &OplogDayHealth,
    staged: &platform::StagedFile,
    original_identity: OplogFileIdentity,
    evidence: &mut OplogCreateEvidence,
) {
    if health.revalidate_publication_ancestors().is_err() {
        evidence.gap(
            OplogEvidenceCheckpoint::FinalBinding,
            OplogGapCause::Changed,
        );
    }
    match platform::inspect_named(health, staged.stage_name.as_os_str()) {
        Ok(_) => {}
        Err(_) => evidence.gap(OplogEvidenceCheckpoint::FinalBinding, OplogGapCause::Io),
    }
    let _ = original_identity;
}

struct FinalLeafResult {
    own_fact: bool,
    conclusive: bool,
}

fn inspect_named_result(
    health: &OplogDayHealth,
    name: &OsStr,
) -> Result<NamedOccupant, OplogGapCause> {
    platform::inspect_named(health, name).map_err(|_| OplogGapCause::Io)
}

fn inspect_final_stage_leaf(
    occupant: Result<NamedOccupant, OplogGapCause>,
    original: OplogFileIdentity,
    stage_name: &OsStr,
    evidence: &mut OplogCreateEvidence,
) -> FinalLeafResult {
    record_leaf_identity(
        occupant,
        original,
        Some(stage_name.to_os_string()),
        OplogEvidenceCheckpoint::FinalStageInspection,
        OplogIdentityObservation::own_noncanonical,
        OplogIdentityObservation::foreign_noncanonical,
        evidence,
    )
}

fn inspect_final_candidate_leaf(
    occupant: Result<NamedOccupant, OplogGapCause>,
    original: OplogFileIdentity,
    dest_name: &OsStr,
    ordinal: u8,
    evidence: &mut OplogCreateEvidence,
) -> FinalLeafResult {
    record_leaf_identity(
        occupant,
        original,
        Some(dest_name.to_os_string()),
        OplogEvidenceCheckpoint::FinalCandidateInspection { ordinal },
        OplogIdentityObservation::own_landed,
        OplogIdentityObservation::foreign_landed,
        evidence,
    )
}

fn observe_stage_leaf(
    health: &OplogDayHealth,
    name: &OsStr,
    original: OplogFileIdentity,
    checkpoint: OplogEvidenceCheckpoint,
    evidence: &mut OplogCreateEvidence,
) {
    let _ = record_leaf_identity(
        inspect_named_result(health, name),
        original,
        Some(name.to_os_string()),
        checkpoint,
        OplogIdentityObservation::own_noncanonical,
        OplogIdentityObservation::foreign_noncanonical,
        evidence,
    );
}

fn record_leaf_identity(
    occupant: Result<NamedOccupant, OplogGapCause>,
    original: OplogFileIdentity,
    native_leaf: Option<OsString>,
    checkpoint: OplogEvidenceCheckpoint,
    own: fn(OplogVerifiedAt) -> OplogIdentityObservation,
    foreign: fn(OplogVerifiedAt) -> OplogIdentityObservation,
    evidence: &mut OplogCreateEvidence,
) -> FinalLeafResult {
    match occupant {
        Ok(NamedOccupant::Regular { identity, nlink: _ }) if identity == original => {
            evidence.observe(own(OplogVerifiedAt::new(native_leaf, checkpoint)));
            FinalLeafResult {
                own_fact: true,
                conclusive: true,
            }
        }
        Ok(NamedOccupant::Regular { .. }) => {
            evidence.observe(foreign(OplogVerifiedAt::new(native_leaf, checkpoint)));
            FinalLeafResult {
                own_fact: false,
                conclusive: true,
            }
        }
        Ok(NamedOccupant::Absent) | Ok(NamedOccupant::Other) => FinalLeafResult {
            own_fact: false,
            conclusive: true,
        },
        Err(cause) => {
            evidence.gap(checkpoint, cause);
            FinalLeafResult {
                own_fact: false,
                conclusive: false,
            }
        }
    }
}

fn aggregate_retained_nlink(
    own_fact_count: u8,
    all_leaves_conclusive: bool,
    retained_nlink: Result<u64, OplogGapCause>,
    evidence: &mut OplogCreateEvidence,
) {
    if !all_leaves_conclusive {
        return;
    }
    let nlink = match retained_nlink {
        Ok(nlink) => nlink,
        Err(cause) => {
            evidence.gap(OplogEvidenceCheckpoint::RetainedHandle, cause);
            return;
        }
    };
    let own_facts = u64::from(own_fact_count);
    if nlink < own_facts {
        evidence.gap(
            OplogEvidenceCheckpoint::RetainedHandle,
            OplogGapCause::Inconsistent,
        );
        return;
    }
    if own_fact_count == 0 && nlink >= 1 {
        evidence.gap(
            OplogEvidenceCheckpoint::RetainedHandle,
            OplogGapCause::NoVerifiedLeaf,
        );
    }
    if nlink > 1 {
        evidence.observe(OplogIdentityObservation::own_multiple_links(
            nlink,
            OplogVerifiedAt::new(None, OplogEvidenceCheckpoint::RetainedHandle),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn classify_failure(
    reason: OplogCreateReason,
    mut evidence: OplogCreateEvidence,
    health: &OplogDayHealth,
    staged: &platform::StagedFile,
    original_identity: OplogFileIdentity,
    attempted: &[(u8, OsString)],
    witnesses: &[CollisionWitness],
) -> OplogCreateError {
    if checkpoint(OplogCreatePrimitive::BeforeFinalFailureClassification).is_err() {
        evidence.gap(
            OplogEvidenceCheckpoint::FinalFailureClassification,
            OplogGapCause::Io,
        );
    }
    barrier(OplogCreatePrimitive::BeforeFinalFailureClassification);
    refresh_collisions(health, &mut evidence, witnesses);
    let mut own_fact_count = 0_u8;
    let mut conclusive = true;
    let stage = inspect_final_stage_leaf(
        inspect_named_result(health, staged.stage_name.as_os_str()),
        original_identity,
        staged.stage_name.as_os_str(),
        &mut evidence,
    );
    if stage.own_fact {
        own_fact_count = own_fact_count.saturating_add(1);
    }
    conclusive &= stage.conclusive;
    for (ordinal, dest) in attempted {
        let candidate = inspect_final_candidate_leaf(
            inspect_named_result(health, dest),
            original_identity,
            dest,
            *ordinal,
            &mut evidence,
        );
        if candidate.own_fact {
            own_fact_count = own_fact_count.saturating_add(1);
        }
        conclusive &= candidate.conclusive;
    }
    let retained_nlink =
        platform::nlink_of(&staged.file).map_err(|_| OplogGapCause::UnobservableHandle);
    aggregate_retained_nlink(own_fact_count, conclusive, retained_nlink, &mut evidence);
    final_binding_evidence_only(health, staged, original_identity, &mut evidence);
    evidence.fail(reason)
}

#[allow(clippy::too_many_arguments)]
fn fail_after_stage(
    reason: OplogCreateReason,
    evidence: OplogCreateEvidence,
    health: &OplogDayHealth,
    staged: &platform::StagedFile,
    original_identity: OplogFileIdentity,
    attempted: &[(u8, OsString)],
    witnesses: &[CollisionWitness],
) -> OplogCreateError {
    classify_failure(
        reason,
        evidence,
        health,
        staged,
        original_identity,
        attempted,
        witnesses,
    )
}

fn refresh_collisions(
    health: &OplogDayHealth,
    evidence: &mut OplogCreateEvidence,
    witnesses: &[CollisionWitness],
) {
    let mut unknown = Vec::new();
    for record in evidence.collisions_mut() {
        let recorded = match record.occupant() {
            OplogCollisionOccupant::Foreign { identity, .. } => *identity,
            _ => continue,
        };
        let dest = record.dest().to_os_string();
        let ordinal = record.ordinal();
        let handle_ok = witnesses.iter().find(|witness| witness.ordinal == ordinal);
        let handle_matches = match handle_ok {
            Some(witness) => platform::identity_of(&witness.file)
                .ok()
                .is_some_and(|identity| identity == recorded && identity == witness.identity),
            None => false,
        };
        if !handle_matches {
            record.set_occupant(OplogCollisionOccupant::Unknown);
            unknown.push(ordinal);
            continue;
        }
        match platform::inspect_named(health, &dest) {
            Ok(NamedOccupant::Regular { identity, .. }) if identity == recorded => {}
            Ok(NamedOccupant::Regular { .. }) => {
                record.set_occupant(OplogCollisionOccupant::Replaced);
            }
            Ok(NamedOccupant::Absent) => record.set_occupant(OplogCollisionOccupant::Absent),
            Ok(NamedOccupant::Other) | Err(_) => {
                record.set_occupant(OplogCollisionOccupant::Unknown);
                unknown.push(ordinal);
            }
        }
    }
    for ordinal in unknown {
        evidence.gap(
            OplogEvidenceCheckpoint::FinalCandidateInspection { ordinal },
            OplogGapCause::Io,
        );
    }
}

fn write_admission_bytes<W: Write>(writer: &mut W, header: &[u8]) -> io::Result<()> {
    let mut written = 0;
    while written < header.len() {
        match writer.write(&header[written..]) {
            Ok(0) => return Err(io::Error::from(ErrorKind::WriteZero)),
            Ok(n) => written += n,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    record_event(OplogCreateEvent::AdmissionBytesAccepted);
    Ok(())
}

fn write_admission(file: &mut std::fs::File, header: &[u8]) -> Result<(), OplogAdmissionCause> {
    write_admission_bytes(file, header).map_err(|_| OplogAdmissionCause::Write)?;
    if force_sync_fail() {
        return Err(OplogAdmissionCause::Sync);
    }
    file.sync_all().map_err(|_| OplogAdmissionCause::Sync)?;
    record_event(OplogCreateEvent::SyncAll);
    Ok(())
}

#[cfg(unix)]
fn nix_or_generic_io() -> i32 {
    nix::libc::EIO
}

fn sync_published(health: &OplogDayHealth) -> io::Result<()> {
    #[cfg(unix)]
    {
        if force_parent_sync_fail() {
            return Err(io::Error::from_raw_os_error(nix_or_generic_io()));
        }
        crate::entry::sync_dir_bound(health.health())
    }
    #[cfg(windows)]
    {
        let _ = health;
        Ok(())
    }
}

fn acquire_lock(
    health: &OplogDayHealth,
    timing: LockTiming,
) -> Result<super::lock::OplogNamespaceLock, OplogCreateReason> {
    let result = match timing {
        LockTiming::Default => acquire_oplog_namespace_lock(health),
        #[cfg(any(test, feature = "test-hooks"))]
        LockTiming::Explicit(timeout, poll_interval) => {
            acquire_oplog_namespace_lock_with_test_timing(health, timeout, poll_interval)
        }
    };
    result.map_err(map_lock_error)
}

fn map_lock_error(error: OplogNamespaceLockError) -> OplogCreateReason {
    OplogCreateReason::Lock(error.create_class())
}

fn map_namespace_error(error: OplogNamespaceError) -> OplogCreateReason {
    OplogCreateReason::Namespace {
        stage: error.create_stage(),
        class: error.create_class(),
    }
}

fn draw_distinct_file_ids() -> Result<[[u8; 16]; OPLOG_CREATE_ATTEMPTS], OplogCreateError> {
    let mut ids = Vec::with_capacity(OPLOG_CREATE_ATTEMPTS);
    let mut seen = HashSet::with_capacity(OPLOG_CREATE_ATTEMPTS);
    for _ in 0..OPLOG_FILE_ID_DRAW_BUDGET {
        let id = draw_file_id()?;
        if seen.insert(id) {
            ids.push(id);
            if ids.len() == OPLOG_CREATE_ATTEMPTS {
                return Ok(ids
                    .try_into()
                    .expect("exactly OPLOG_CREATE_ATTEMPTS distinct ids"));
            }
        }
    }
    Err(bare(OplogCreateReason::EntropyExhausted))
}

fn draw_file_id() -> Result<[u8; 16], OplogCreateError> {
    fill_oplog_file_id()
}

fn fill_oplog_file_id() -> Result<[u8; 16], OplogCreateError> {
    record_event(OplogCreateEvent::EntropyDraw);
    if take_entropy_source_fault() {
        return Err(bare(OplogCreateReason::EntropySource));
    }
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(bytes) = take_injected_file_id() {
        return Ok(bytes);
    }
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| bare(OplogCreateReason::EntropySource))?;
    Ok(bytes)
}

/// Bound no-follow lease probe of one leaf under the admitted day-health directory.
pub fn probe_oplog_lease(health: &OplogDayHealth, leaf: &OsStr) -> LeaseProbe {
    if force_probe_indeterminate() {
        return LeaseProbe::Indeterminate;
    }
    platform::probe_named(health, leaf)
}

/// Ordered checkpoints for one create call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogCreatePrimitive {
    /// Exclusive stage allocation.
    Stage,
    /// After exclusive allocate, before permission/append/identity prepare.
    AfterAllocateBeforePrepare,
    /// Admission-record write and file sync.
    Admission,
    /// After staging, before taking the self-lease.
    AfterStageBeforeLease,
    /// Self-lease acquisition.
    Lease,
    /// After lease, before the dest-rename loop.
    AfterLeaseBeforePublish,
    /// Per-candidate no-replace rename syscall.
    Rename,
    /// After a successful kernel rename, before dest inspect.
    AfterRename,
    /// After a proven-foreign dest collision.
    AfterForeignCollision,
    /// After a landed rename, before directory sync.
    AfterRenameBeforeDirectorySync,
    /// Immediately before final failure classification.
    BeforeFinalFailureClassification,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogCreateEvent {
    EntropyDraw,
    AdmissionBytesAccepted,
    SyncAll,
    Lease,
    Publish,
}

#[cfg(any(test, feature = "test-hooks"))]
struct OplogCreateTraceState {
    fault: Option<(OplogCreatePrimitive, usize)>,
    fault_consumed: bool,
    attempted: Vec<OplogCreatePrimitive>,
    barriers: Vec<(OplogCreatePrimitive, Box<dyn FnOnce()>)>,
    file_ids: std::collections::VecDeque<[u8; 16]>,
    probe_indeterminate: bool,
    dest_identity_io: bool,
    publish_io: bool,
    sync_fail: bool,
    #[cfg(unix)]
    parent_sync_fail: bool,
    #[cfg(unix)]
    stage_permission_fail: bool,
    #[cfg(unix)]
    stage_append_fail: bool,
    stage_identity_fail: bool,
    entropy_fault: Option<usize>,
    entropy_fault_consumed: bool,
    entropy_draws: usize,
    events: Vec<OplogCreateEvent>,
    sampled_instant: Option<DateTime<FixedOffset>>,
    sampler_fail: bool,
    sampler_calls: usize,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static OPLOG_CREATE_TRACE: std::cell::RefCell<Option<OplogCreateTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn checkpoint(_primitive: OplogCreatePrimitive) -> Result<(), OplogCreateReason> {
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint(primitive: OplogCreatePrimitive) -> Result<(), OplogCreateReason> {
    let fault = OPLOG_CREATE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return false;
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == primitive)
            .count();
        if state.fault == Some((primitive, ordinal)) {
            state.fault = None;
            state.fault_consumed = true;
            true
        } else {
            false
        }
    });
    if fault {
        return Err(match primitive {
            OplogCreatePrimitive::Lease => OplogCreateReason::LeaseFailed,
            OplogCreatePrimitive::Stage => OplogCreateReason::Stage(OplogStageCause::Allocate),
            OplogCreatePrimitive::Admission => {
                OplogCreateReason::Admission(OplogAdmissionCause::Write)
            }
            OplogCreatePrimitive::Rename => OplogCreateReason::Publish(OplogPublishReason::Rename),
            OplogCreatePrimitive::AfterRename
            | OplogCreatePrimitive::AfterForeignCollision
            | OplogCreatePrimitive::AfterRenameBeforeDirectorySync => {
                OplogCreateReason::Publish(OplogPublishReason::DestinationInspection)
            }
            OplogCreatePrimitive::BeforeFinalFailureClassification => {
                OplogCreateReason::Publish(OplogPublishReason::DestinationExhaustion)
            }
            _ => OplogCreateReason::Publish(OplogPublishReason::Rename),
        });
    }
    Ok(())
}

#[cfg(not(any(test, feature = "test-hooks")))]
pub(super) fn barrier(_primitive: OplogCreatePrimitive) {}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn barrier(primitive: OplogCreatePrimitive) {
    let callback = OPLOG_CREATE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state
            .barriers
            .iter()
            .position(|(candidate, _)| *candidate == primitive)
            .map(|index| state.barriers.remove(index).1)
    });
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(all(unix, not(any(test, feature = "test-hooks"))))]
fn force_parent_sync_fail() -> bool {
    false
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
fn force_parent_sync_fail() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.parent_sync_fail)
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn force_probe_indeterminate() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn force_probe_indeterminate() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.probe_indeterminate)
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
pub(super) fn force_dest_identity_io() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn force_dest_identity_io() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.dest_identity_io)
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
pub(super) fn force_publish_io() -> bool {
    false
}

#[cfg(all(unix, not(any(test, feature = "test-hooks"))))]
pub(super) fn force_stage_permission_fail() -> bool {
    false
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub(super) fn force_stage_permission_fail() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.stage_permission_fail)
    })
}

#[cfg(all(unix, not(any(test, feature = "test-hooks"))))]
pub(super) fn force_stage_append_fail() -> bool {
    false
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub(super) fn force_stage_append_fail() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.stage_append_fail)
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
pub(super) fn force_stage_identity_fail() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn force_stage_identity_fail() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.stage_identity_fail)
    })
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn force_publish_io() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow()
            .as_ref()
            .is_some_and(|state| state.publish_io)
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn force_sync_fail() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn force_sync_fail() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| trace.borrow().as_ref().is_some_and(|state| state.sync_fail))
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn take_entropy_source_fault() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn take_entropy_source_fault() -> bool {
    OPLOG_CREATE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return false;
        };
        state.entropy_draws += 1;
        if state.entropy_fault == Some(state.entropy_draws) {
            state.entropy_fault = None;
            state.entropy_fault_consumed = true;
            true
        } else {
            false
        }
    })
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn record_event(_event: OplogCreateEvent) {}

#[cfg(any(test, feature = "test-hooks"))]
fn record_event(event: OplogCreateEvent) {
    OPLOG_CREATE_TRACE.with(|trace| {
        if let Some(state) = trace.borrow_mut().as_mut() {
            state.events.push(event);
        }
    });
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn take_sampler_override() -> Option<Result<DateTime<FixedOffset>, OplogCreateError>> {
    OPLOG_CREATE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state.sampler_calls += 1;
        if state.sampler_fail {
            Some(Err(bare(OplogCreateReason::Clock)))
        } else {
            state.sampled_instant.map(Ok)
        }
    })
}

#[cfg(any(test, feature = "test-hooks"))]
fn take_injected_file_id() -> Option<[u8; 16]> {
    OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .as_mut()
            .and_then(|state| state.file_ids.pop_front())
    })
}

#[cfg(any(test, feature = "test-hooks"))]
fn empty_trace() -> OplogCreateTraceState {
    OplogCreateTraceState {
        fault: None,
        fault_consumed: false,
        attempted: Vec::new(),
        barriers: Vec::new(),
        file_ids: std::collections::VecDeque::new(),
        probe_indeterminate: false,
        dest_identity_io: false,
        publish_io: false,
        sync_fail: false,
        #[cfg(unix)]
        parent_sync_fail: false,
        #[cfg(unix)]
        stage_permission_fail: false,
        #[cfg(unix)]
        stage_append_fail: false,
        stage_identity_fail: false,
        entropy_fault: None,
        entropy_fault_consumed: false,
        entropy_draws: 0,
        events: Vec::new(),
        sampled_instant: None,
        sampler_fail: false,
        sampler_calls: 0,
    }
}

/// Run `operation` with one injected create fault at the first occurrence.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_create_fault<T>(
    primitive: OplogCreatePrimitive,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    run_with_oplog_create_fault_at(primitive, 1, operation)
}

/// Run `operation` with one injected create fault at a 1-based primitive ordinal.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_create_fault_at<T>(
    primitive: OplogCreatePrimitive,
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, state) = with_trace(
        OplogCreateTraceState {
            fault: Some((primitive, ordinal)),
            ..empty_trace()
        },
        operation,
    );
    (result, state.fault_consumed)
}

/// Run `operation` with one barrier callback.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_create_barrier<T>(
    primitive: OplogCreatePrimitive,
    callback: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    with_trace(
        OplogCreateTraceState {
            barriers: vec![(primitive, Box::new(callback))],
            ..empty_trace()
        },
        operation,
    )
    .0
}

/// Inject file ids consumed in order before falling back to `getrandom`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_file_ids<T>(ids: Vec<[u8; 16]>, operation: impl FnOnce() -> T) -> T {
    with_trace(
        OplogCreateTraceState {
            file_ids: ids.into(),
            ..empty_trace()
        },
        operation,
    )
    .0
}

/// Force `probe_oplog_lease` to return `Indeterminate`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_probe_indeterminate<T>(operation: impl FnOnce() -> T) -> T {
    with_trace(
        OplogCreateTraceState {
            probe_indeterminate: true,
            ..empty_trace()
        },
        operation,
    )
    .0
}

/// Run `operation` with one entropy-adapter fault at the first draw.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_entropy_source_fault<T>(operation: impl FnOnce() -> T) -> (T, bool) {
    run_with_oplog_entropy_source_fault_at(1, operation)
}

/// Run `operation` with one entropy-adapter fault at a 1-based draw ordinal.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_entropy_source_fault_at<T>(
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, state) = with_trace(
        OplogCreateTraceState {
            entropy_fault: Some(ordinal),
            ..empty_trace()
        },
        operation,
    );
    (result, state.entropy_fault_consumed)
}

/// Freeze the production sampler to `instant`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_sampled_instant<T>(
    instant: DateTime<FixedOffset>,
    operation: impl FnOnce() -> T,
) -> T {
    with_trace(
        OplogCreateTraceState {
            sampled_instant: Some(instant),
            ..empty_trace()
        },
        operation,
    )
    .0
}

/// Fail the production sampler before any entropy draw.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_sampler_fault<T>(operation: impl FnOnce() -> T) -> (T, bool) {
    let (result, state) = with_trace(
        OplogCreateTraceState {
            sampler_fail: true,
            ..empty_trace()
        },
        operation,
    );
    (result, state.sampler_calls > 0)
}

/// Fail `sync_all` after the admission bytes are written.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_sync_fail<T>(operation: impl FnOnce() -> T) -> T {
    with_trace(
        OplogCreateTraceState {
            sync_fail: true,
            ..empty_trace()
        },
        operation,
    )
    .0
}

/// Fail the retained-directory durability sync after a landed rename.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_oplog_parent_sync_fail<T>(operation: impl FnOnce() -> T) -> T {
    with_trace(
        OplogCreateTraceState {
            parent_sync_fail: true,
            ..empty_trace()
        },
        operation,
    )
    .0
}

#[cfg(any(test, feature = "test-hooks"))]
fn with_trace<T>(
    state: OplogCreateTraceState,
    operation: impl FnOnce() -> T,
) -> (T, OplogCreateTraceState) {
    OPLOG_CREATE_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "oplog create trace is already active"
        );
        *trace.borrow_mut() = Some(state);
    });
    let result = operation();
    let state = OPLOG_CREATE_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("oplog create trace remains active")
    });
    (result, state)
}

#[cfg(all(test, unix))]
fn spawn_sleep_holding_oplog_stdout(stdio: std::process::Stdio) -> std::process::Child {
    use std::process::{Command, Stdio};

    Command::new("sleep")
        .arg("0.3")
        .stdout(stdio)
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[cfg(all(test, unix))]
mod tests {
    use std::error::Error;
    use std::ffi::OsStr;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    use chrono::DateTime;
    use nix::sys::stat::{Mode, umask};

    use super::*;
    use crate::journal_root::JournalRoot;
    use crate::lease::{DEFAULT_LEASE_RETRY_MAX, probe_file_lease};
    use crate::operational_log::name::{OplogNameClassification, classify_oplog_name};
    use crate::operational_log::{
        OplogAncestorComponent, OplogCollisionOccupant, OplogCreateError, OplogEvidenceCheckpoint,
        OplogFileIdentity, OplogFormat, OplogGapCause, OplogIdentityObservation,
        OplogNamespacePrimitive, RetainedNamespaceState,
        acquire_oplog_namespace_lock_with_test_timing, admit_day_health_directory,
        run_with_oplog_namespace_barrier, run_with_oplog_namespace_fault, validate_oplog_admission,
    };

    const ZERO: Duration = Duration::ZERO;
    const SOURCE: &str = "cortex";
    const RUN: &str = "daily-think";

    fn instant() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-09-01T16:42:33.381904Z").unwrap()
    }

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir_in("/var/tmp").unwrap()
    }

    fn health_at(root: &Path) -> crate::operational_log::OplogDayHealth {
        let (day, _) = derive_day_key_and_opened_field(instant());
        admit_day_health_directory(JournalRoot::open(root).unwrap(), &day).unwrap()
    }

    fn create(root: &Path) -> Result<OplogWriter, OplogCreateError> {
        create_oplog_with_test_timing(
            JournalRoot::open(root).unwrap(),
            SOURCE,
            RUN,
            OplogFormat::Log,
            instant(),
            ZERO,
            ZERO,
        )
    }

    fn dest_for(file_id: [u8; 16]) -> String {
        let (_, opened) = derive_day_key_and_opened_field(instant());
        format_oplog_name(&oplog_name_from_parts(
            SOURCE,
            RUN,
            opened,
            file_id_hex(&file_id),
            OplogFormat::Log,
        ))
    }

    fn health_dir(root: &Path) -> std::path::PathBuf {
        let (day, _) = derive_day_key_and_opened_field(instant());
        root.join("chronicle").join(day).join("health")
    }

    fn expect_token(error: &OplogCreateError, token: &str) {
        assert_eq!(error.to_string(), token);
        assert_eq!(format!("{error:?}"), token);
        assert!(error.source().is_none());
    }

    fn count_event(state: &OplogCreateTraceState, event: OplogCreateEvent) -> usize {
        state
            .events
            .iter()
            .filter(|candidate| **candidate == event)
            .count()
    }

    // A concurrent test's forked child can briefly inherit this writer's OFD across
    // fork-to-exec (CLOEXEC applies at exec, not fork); the lock self-releases.
    fn assert_lease_released(health: &OplogDayHealth, leaf: &OsStr) {
        let deadline = std::time::Instant::now() + DEFAULT_LEASE_RETRY_MAX;
        loop {
            if probe_oplog_lease(health, leaf) == LeaseProbe::Released {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "lease did not converge to Released within {DEFAULT_LEASE_RETRY_MAX:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn listing(path: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn canonical_leaves(path: &Path) -> Vec<String> {
        listing(path)
            .into_iter()
            .filter(|name| {
                matches!(
                    classify_oplog_name(OsStr::new(name)),
                    OplogNameClassification::Candidate(Ok(_))
                )
            })
            .collect()
    }

    #[cfg(unix)]
    fn open_fd_count_for(path: &Path) -> usize {
        let target = match fs::canonicalize(path) {
            Ok(path) => path,
            Err(_) => return 0,
        };
        fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                fs::read_link(entry.path())
                    .ok()
                    .is_some_and(|linked| linked == target)
            })
            .count()
    }

    fn leftover_unrelated(path: &Path) -> Vec<String> {
        listing(path)
            .into_iter()
            .filter(|name| *name != ".oplog-namespace.lock")
            .filter(|name| {
                matches!(
                    classify_oplog_name(OsStr::new(name)),
                    OplogNameClassification::Unrelated
                )
            })
            .collect()
    }

    fn payload_after_admission(path: &Path, leaf: &str) -> Vec<u8> {
        let bytes = fs::read(path).unwrap();
        let record = validate_oplog_admission(OsStr::new(leaf), &bytes).unwrap();
        bytes[record.header_len()..].to_vec()
    }

    #[test]
    fn two_creates_at_the_same_instant_get_distinct_file_ids() {
        let temporary = temp();
        let first = create(temporary.path()).unwrap();
        let second = create(temporary.path()).unwrap();
        assert_ne!(first.leaf_name(), second.leaf_name());
        let dir = health_dir(temporary.path());
        assert_eq!(canonical_leaves(&dir).len(), 2);
    }

    #[test]
    fn injected_file_id_collision_retries_without_touching_incumbent() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let first_id = [0x11; 16];
        let second_id = [0x22; 16];
        let incumbent = dest_for(first_id);
        let path = health_dir(temporary.path()).join(&incumbent);
        fs::write(&path, b"incumbent").unwrap();
        let writer =
            run_with_oplog_file_ids(vec![first_id, second_id], || create(temporary.path()))
                .unwrap();
        assert_eq!(writer.leaf_name(), dest_for(second_id));
        assert_eq!(fs::read(&path).unwrap(), b"incumbent");
    }

    #[test]
    fn exhausted_collisions_leave_incumbents_byte_identical() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let dir = health_dir(temporary.path());
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [index as u8; 16])
            .collect();
        let incumbents: Vec<String> = ids.iter().copied().map(dest_for).collect();
        for incumbent in &incumbents {
            fs::write(dir.join(incumbent), b"same-bytes").unwrap();
        }
        let error = run_with_oplog_file_ids(ids, || create(temporary.path())).unwrap_err();
        expect_token(&error, "oplog_create_destination_exhaustion");
        for incumbent in &incumbents {
            assert_eq!(fs::read(dir.join(incumbent)).unwrap(), b"same-bytes");
        }
        assert_eq!(canonical_leaves(&dir), incumbents);
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn random_source_failure_does_not_retry() {
        let temporary = temp();
        let (result, consumed) = run_with_oplog_entropy_source_fault(|| create(temporary.path()));
        assert!(consumed);
        expect_token(&result.unwrap_err(), "oplog_create_entropy_source");
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn sixty_four_duplicate_ids_are_entropy_exhausted_with_zero_side_effects() {
        let temporary = temp();
        let id = [0x11; 16];
        let error = run_with_oplog_file_ids(vec![id; OPLOG_FILE_ID_DRAW_BUDGET], || {
            create(temporary.path())
        })
        .unwrap_err();
        expect_token(&error, "oplog_create_entropy_exhausted");
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn non_collision_errors_return_immediately() {
        let temporary = temp();
        for (primitive, token) in [
            (OplogCreatePrimitive::Stage, "oplog_create_stage_allocate"),
            (OplogCreatePrimitive::Rename, "oplog_create_rename"),
        ] {
            let (result, consumed) =
                run_with_oplog_create_fault(primitive, || create(temporary.path()));
            assert!(consumed);
            expect_token(&result.unwrap_err(), token);
            assert!(canonical_leaves(&health_dir(temporary.path())).is_empty());
        }
    }

    #[test]
    fn lease_failure_rolls_back_only_the_stage() {
        let temporary = temp();
        let (result, consumed) =
            run_with_oplog_create_fault(OplogCreatePrimitive::Lease, || create(temporary.path()));
        assert!(consumed);
        expect_token(&result.unwrap_err(), "oplog_create_lease_failed");
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert!(listing(&dir).contains(&".oplog-namespace.lock".to_owned()));
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn lease_failure_leaves_unrelated_native_name() {
        let temporary = temp();
        let error = with_trace(
            OplogCreateTraceState {
                fault: Some((OplogCreatePrimitive::Lease, 1)),
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_lease_failed");
        let names = listing(&health_dir(temporary.path()));
        let residue = names
            .iter()
            .find(|name| *name != ".oplog-namespace.lock")
            .expect("stage residue remains");
        assert!(matches!(
            classify_oplog_name(OsStr::new(residue)),
            OplogNameClassification::Unrelated
        ));
        assert!(canonical_leaves(&health_dir(temporary.path())).is_empty());
    }

    #[test]
    fn barriers_see_no_canonical_candidate_and_lock_blocks_a_second_publisher() {
        for primitive in [
            OplogCreatePrimitive::AfterStageBeforeLease,
            OplogCreatePrimitive::AfterLeaseBeforePublish,
        ] {
            let isolated = temp();
            let root = isolated.path().to_path_buf();
            run_with_oplog_create_barrier(
                primitive,
                {
                    let root = root.clone();
                    move || {
                        let dir = health_dir(&root);
                        assert!(canonical_leaves(&dir).is_empty());
                        let second = health_at(&root);
                        let error =
                            acquire_oplog_namespace_lock_with_test_timing(&second, ZERO, ZERO)
                                .unwrap_err();
                        assert_eq!(error.to_string(), "oplog_namespace_lock_busy");
                    }
                },
                {
                    let root = root.clone();
                    move || create(&root)
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn probe_is_active_after_publish_and_released_after_drop() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let writer = create(temporary.path()).unwrap();
        let leaf = writer.leaf_name().to_owned();
        assert_eq!(
            probe_oplog_lease(&health, OsStr::new(&leaf)),
            LeaseProbe::Active
        );
        drop(writer);
        assert_lease_released(&health, OsStr::new(&leaf));
    }

    #[test]
    fn published_file_starts_with_admission_header_then_payload() {
        let temporary = temp();
        let mut writer = create(temporary.path()).unwrap();
        writer.write_all(b"payload-line\n").unwrap();
        writer.flush().unwrap();
        let leaf = writer.leaf_name().to_owned();
        drop(writer);
        let bytes = fs::read(health_dir(temporary.path()).join(&leaf)).unwrap();
        let record = validate_oplog_admission(OsStr::new(&leaf), &bytes).unwrap();
        assert_eq!(&bytes[record.header_len()..], b"payload-line\n");
    }

    #[test]
    fn admission_fault_leaves_unrelated_stage() {
        let temporary = temp();
        let (result, consumed) =
            run_with_oplog_create_fault(OplogCreatePrimitive::Admission, || {
                create(temporary.path())
            });
        assert!(consumed);
        expect_token(&result.unwrap_err(), "oplog_create_admission_write");
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[cfg(unix)]
    fn stage_prepare_fault(
        fail: impl Fn(&mut OplogCreateTraceState),
    ) -> (OplogCreateError, tempfile::TempDir) {
        let temporary = temp();
        let error = with_trace(
            {
                let mut state = empty_trace();
                fail(&mut state);
                state
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        (error, temporary)
    }

    #[cfg(unix)]
    #[test]
    fn stage_permission_fault_after_allocate_is_stage_permission() {
        let (error, temporary) = stage_prepare_fault(|state| state.stage_permission_fail = true);
        expect_token(&error, "oplog_create_stage_permission");
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn stage_append_fault_after_allocate_is_stage_append() {
        let (error, temporary) = stage_prepare_fault(|state| state.stage_append_fail = true);
        expect_token(&error, "oplog_create_stage_append");
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn stage_identity_fault_after_allocate_is_stage_identity() {
        let temporary = temp();
        let error = with_trace(
            OplogCreateTraceState {
                stage_identity_fail: true,
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_stage_identity");
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn after_allocate_before_prepare_stage_has_zero_length() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterAllocateBeforePrepare,
            {
                let dir = health_dir(&root);
                move || {
                    let leftover = leftover_unrelated(&dir);
                    assert_eq!(leftover.len(), 1);
                    assert_eq!(fs::metadata(dir.join(&leftover[0])).unwrap().len(), 0);
                    assert!(canonical_leaves(&dir).is_empty());
                }
            },
            || create(&root),
        )
        .unwrap();
    }

    #[test]
    fn extra_hard_link_before_publish_is_aliased() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterLeaseBeforePublish,
            {
                let dir = health_dir(&root);
                move || {
                    let stage = fs::read_dir(&dir)
                        .unwrap()
                        .map(|entry| entry.unwrap().file_name())
                        .find(|name| name.to_string_lossy().contains(".tmp"))
                        .expect("stage name");
                    fs::hard_link(dir.join(&stage), dir.join("alias-link")).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_alias");
        let dir = health_dir(&root);
        assert_eq!(canonical_leaves(&dir).len(), 1);
        assert!(dir.join("alias-link").exists());
    }

    #[test]
    fn stage_pathname_replacement_is_foreign_residue() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let id = [0x44; 16];
        let dest = dest_for(id);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: vec![id].into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterLeaseBeforePublish,
                    Box::new({
                        let dir = health_dir(&root);
                        move || {
                            let stage = fs::read_dir(&dir)
                                .unwrap()
                                .map(|entry| entry.unwrap().file_name())
                                .find(|name| name.to_string_lossy().contains(".tmp"))
                                .expect("stage name");
                            let from = dir.join(&stage);
                            fs::rename(&from, dir.join("displaced-stage")).unwrap();
                            fs::write(&from, b"replacement").unwrap();
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_destination_inspection");
        let dir = health_dir(&root);
        assert_eq!(fs::read(dir.join(&dest)).unwrap(), b"replacement");
        let displaced = fs::read(dir.join("displaced-stage")).unwrap();
        assert!(displaced.starts_with(b"{\"_solstone_oplog_v\":1"));
    }

    #[test]
    fn dest_identity_io_after_publish_is_own_residue_and_preserves_dest() {
        let temporary = temp();
        let id = [0x88; 16];
        let dest = dest_for(id);
        let error = with_trace(
            OplogCreateTraceState {
                dest_identity_io: true,
                file_ids: vec![id].into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_destination_inspection");
        let path = health_dir(temporary.path()).join(&dest);
        let bytes = fs::read(&path).unwrap();
        let record = validate_oplog_admission(OsStr::new(&dest), &bytes).unwrap();
        assert_eq!(&bytes[record.header_len()..], b"");
    }

    #[test]
    fn publish_io_leaves_unrelated_stage_residue() {
        let temporary = temp();
        let error = with_trace(
            OplogCreateTraceState {
                publish_io: true,
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_reconciliation");
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn occupied_retries_leave_unrelated_stage_residue() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let dir = health_dir(temporary.path());
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x66 + index as u8; 16])
            .collect();
        let incumbents: Vec<String> = ids.iter().copied().map(dest_for).collect();
        for incumbent in &incumbents {
            fs::write(dir.join(incumbent), b"preexisting").unwrap();
        }
        let error = run_with_oplog_file_ids(ids, || create(temporary.path())).unwrap_err();
        expect_token(&error, "oplog_create_destination_exhaustion");
        for incumbent in &incumbents {
            assert_eq!(fs::read(dir.join(incumbent)).unwrap(), b"preexisting");
        }
        assert_eq!(canonical_leaves(&dir), incumbents);
        assert_eq!(leftover_unrelated(&dir).len(), 1);
        assert_eq!(error.collisions().len(), OPLOG_CREATE_ATTEMPTS);
        assert_eq!(
            error
                .observations()
                .iter()
                .filter(|observation| matches!(
                    observation,
                    OplogIdentityObservation::ForeignLanded(verified)
                        if matches!(
                            verified.checkpoint(),
                            OplogEvidenceCheckpoint::AfterForeignCollision { .. }
                        )
                ))
                .count(),
            OPLOG_CREATE_ATTEMPTS
        );
        assert_eq!(
            error
                .observations()
                .iter()
                .filter(|observation| matches!(
                    observation,
                    OplogIdentityObservation::ForeignLanded(verified)
                        if matches!(
                            verified.checkpoint(),
                            OplogEvidenceCheckpoint::FinalCandidateInspection { .. }
                        )
                ))
                .count(),
            OPLOG_CREATE_ATTEMPTS
        );
        assert_eq!(
            error
                .observations()
                .iter()
                .filter(|observation| matches!(
                    observation,
                    OplogIdentityObservation::OwnNoncanonical(_)
                ))
                .count(),
            1
        );
    }

    #[test]
    fn namespace_lock_excludes_and_releases() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let first = acquire_oplog_namespace_lock_with_test_timing(&health, ZERO, ZERO).unwrap();
        assert_eq!(
            acquire_oplog_namespace_lock_with_test_timing(&health, ZERO, ZERO)
                .unwrap_err()
                .to_string(),
            "oplog_namespace_lock_busy"
        );
        drop(first);
        drop(acquire_oplog_namespace_lock_with_test_timing(&health, ZERO, ZERO).unwrap());
    }

    #[test]
    fn unsafe_lock_fails_before_stage() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let lock_path = health_dir(temporary.path()).join(".oplog-namespace.lock");
        std::os::unix::fs::symlink("outside", &lock_path).unwrap();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x55 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.clone().into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        expect_token(&result.unwrap_err(), "oplog_create_lock_unsafe");
        assert_eq!(state.file_ids.len(), 0);
        for id in ids {
            assert!(!health_dir(temporary.path()).join(dest_for(id)).exists());
        }
    }

    #[test]
    fn wrong_mode_lock_fails_before_stage() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let lock_path = health_dir(temporary.path()).join(".oplog-namespace.lock");
        fs::write(&lock_path, b"unchanged").unwrap();
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).unwrap();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x56 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.clone().into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        expect_token(&result.unwrap_err(), "oplog_create_lock_unsafe");
        assert_eq!(state.file_ids.len(), 0);
        for id in ids {
            assert!(!health_dir(temporary.path()).join(dest_for(id)).exists());
        }
        assert_eq!(fs::read(&lock_path).unwrap(), b"unchanged");
    }

    #[test]
    fn replaced_lock_parent_fails_before_stage() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let dir = health_dir(temporary.path());
        let lock_path = dir.join(".oplog-namespace.lock");
        fs::write(&lock_path, b"original-lock").unwrap();
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(&lock_path).unwrap();
        fs::create_dir(&lock_path).unwrap();
        // Bound acquire never re-opens the day-health parent by path, so a
        // pathname swap of that directory cannot produce ParentChanged. The
        // replacement the primitive *can* refuse before stage is a lock
        // entry whose identity/kind no longer matches a 0o600 regular file
        // — here the lock name is replaced with a directory, the same
        // shape as cortex_use::lock's "directory" unsafe-entry fixture.
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x77 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.clone().into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        expect_token(&result.unwrap_err(), "oplog_create_lock_unsafe");
        assert_eq!(state.file_ids.len(), 0);
        for id in ids {
            assert!(!dir.join(dest_for(id)).exists());
        }
        assert!(lock_path.is_dir());
    }

    #[test]
    fn mid_flight_ancestor_replacement_does_not_escape() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let outside = root.join("escape-target");
        fs::create_dir(&outside).unwrap();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterLeaseBeforePublish,
            {
                let root = root.clone();
                let outside = outside.clone();
                move || {
                    let dir = health_dir(&root);
                    fs::rename(&dir, root.join("health-displaced")).unwrap();
                    std::os::unix::fs::symlink(&outside, &dir).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_ancestor_revalidation");
        let displaced = root.join("health-displaced");
        assert!(canonical_leaves(&displaced).is_empty());
        assert!(canonical_leaves(&outside).is_empty());
        assert_eq!(leftover_unrelated(&displaced).len(), 1);
        assert!(
            fs::symlink_metadata(health_dir(&root))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn production_create_rejects_invalid_originals_without_side_effects() {
        let temporary = temp();
        expect_token(
            &create_oplog(
                JournalRoot::open(temporary.path()).unwrap(),
                "",
                RUN,
                OplogFormat::Log,
            )
            .unwrap_err(),
            "oplog_create_invalid_field",
        );
        expect_token(
            &create_oplog(
                JournalRoot::open(temporary.path()).unwrap(),
                "ok",
                "bad\0name",
                OplogFormat::Log,
            )
            .unwrap_err(),
            "oplog_create_invalid_field",
        );
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn invalid_originals_and_wrong_kind_namespace_do_not_create() {
        let temporary = temp();
        expect_token(
            &create_oplog_with_test_timing(
                JournalRoot::open(temporary.path()).unwrap(),
                "",
                RUN,
                OplogFormat::Log,
                instant(),
                ZERO,
                ZERO,
            )
            .unwrap_err(),
            "oplog_create_invalid_field",
        );
        expect_token(
            &create_oplog_with_test_timing(
                JournalRoot::open(temporary.path()).unwrap(),
                "ok",
                "bad\0name",
                OplogFormat::Log,
                instant(),
                ZERO,
                ZERO,
            )
            .unwrap_err(),
            "oplog_create_invalid_field",
        );
        assert!(!temporary.path().join("chronicle").exists());
        let root = temporary.path().join("other");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink("elsewhere", root.join("chronicle")).unwrap();
        let error =
            admit_day_health_directory(JournalRoot::open(&root).unwrap(), "20260901").unwrap_err();
        assert_eq!(error.to_string(), "oplog_namespace_chronicle_unsafe");
        assert!(!root.join("elsewhere").exists());
    }

    #[test]
    fn reader_survives_pathname_replacement_and_drop_releases() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let mut writer = create(temporary.path()).unwrap();
        writer.write_all(b"hello\n").unwrap();
        writer.flush().unwrap();
        let dir = health_dir(temporary.path());
        let leaf = writer.leaf_name().to_owned();
        let path = dir.join(&leaf);
        let retained = fs::File::open(&path).unwrap();
        fs::rename(&path, dir.join("moved")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        writer.write_all(b"late\n").unwrap();
        writer.flush().unwrap();
        assert_eq!(probe_file_lease(&retained), LeaseProbe::Active);
        assert_eq!(
            payload_after_admission(&dir.join("moved"), &leaf),
            b"hello\nlate\n"
        );
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        drop(writer);
        assert_lease_released(&health, OsStr::new("moved"));
    }

    #[test]
    fn owner_only_mode_under_permissive_umask() {
        let temporary = temp();
        let previous = umask(Mode::from_bits_truncate(0o022));
        let writer = create(temporary.path()).unwrap();
        umask(previous);
        let mode = fs::metadata(health_dir(temporary.path()).join(writer.leaf_name()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn stdio_duplicates_append_exactly_once() {
        let temporary = temp();
        let mut writer = create(temporary.path()).unwrap();
        let mut dup = writer.try_clone_for_stdio().unwrap();
        writer.write_all(b"one\n").unwrap();
        dup.write_all(b"two\n").unwrap();
        writer.write_all(b"three\n").unwrap();
        writer.flush().unwrap();
        dup.flush().unwrap();
        let bytes = payload_after_admission(
            &health_dir(temporary.path()).join(writer.leaf_name()),
            writer.leaf_name(),
        );
        assert_eq!(bytes, b"one\ntwo\nthree\n");
    }

    #[test]
    fn dropping_writer_keeps_duplicate_active() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let writer = create(temporary.path()).unwrap();
        let mut dup = writer.try_clone_for_stdio().unwrap();
        let leaf = writer.leaf_name().to_owned();
        drop(writer);
        assert_eq!(
            probe_oplog_lease(&health, OsStr::new(&leaf)),
            LeaseProbe::Active
        );
        dup.write_all(b"still\n").unwrap();
        dup.flush().unwrap();
        drop(dup);
        assert_lease_released(&health, OsStr::new(&leaf));
        assert_eq!(
            payload_after_admission(&health_dir(temporary.path()).join(&leaf), &leaf),
            b"still\n"
        );
    }

    #[test]
    fn child_process_exit_releases_lease() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let writer = create(temporary.path()).unwrap();
        let leaf = writer.leaf_name().to_owned();
        let stdio = writer.duplicate_locked_stdio().unwrap();
        drop(writer);
        let mut child = super::spawn_sleep_holding_oplog_stdout(stdio);
        assert_eq!(
            probe_oplog_lease(&health, OsStr::new(&leaf)),
            LeaseProbe::Active
        );
        let status = child.wait().unwrap();
        assert!(status.success());
        drop(child);
        assert_lease_released(&health, OsStr::new(&leaf));
    }

    #[test]
    fn injected_probe_failure_is_indeterminate_without_companion_files() {
        let temporary = temp();
        let health = health_at(temporary.path());
        let writer = create(temporary.path()).unwrap();
        let leaf = OsStr::new(writer.leaf_name());
        let probe = run_with_oplog_probe_indeterminate(|| probe_oplog_lease(&health, leaf));
        assert_eq!(probe, LeaseProbe::Indeterminate);
        let names = listing(&health_dir(temporary.path()));
        assert!(names.contains(&".oplog-namespace.lock".to_owned()));
        assert!(names.contains(&writer.leaf_name().to_owned()));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn day_and_utc_come_from_the_same_instant() {
        let temporary = temp();
        let offset = DateTime::parse_from_rfc3339("2026-09-01T23:30:00-05:00").unwrap();
        let (day, opened) = derive_day_key_and_opened_field(offset);
        let writer = create_oplog_with_test_timing(
            JournalRoot::open(temporary.path()).unwrap(),
            SOURCE,
            RUN,
            OplogFormat::Log,
            offset,
            ZERO,
            ZERO,
        )
        .unwrap();
        assert!(writer.leaf_name().contains(&opened));
        assert!(
            temporary
                .path()
                .join("chronicle")
                .join(&day)
                .join("health")
                .exists()
        );
    }

    #[test]
    fn entropy_fault_at_every_draw_ordinal_leaves_no_chronicle() {
        for ordinal in 1..=OPLOG_FILE_ID_DRAW_BUDGET {
            let temporary = temp();
            let (result, state) = with_trace(
                OplogCreateTraceState {
                    entropy_fault: Some(ordinal),
                    file_ids: vec![[0x11; 16]; OPLOG_FILE_ID_DRAW_BUDGET].into(),
                    ..empty_trace()
                },
                || create(temporary.path()),
            );
            assert!(state.entropy_fault_consumed, "ordinal {ordinal}");
            expect_token(&result.unwrap_err(), "oplog_create_entropy_source");
            assert_eq!(
                state.file_ids.len(),
                OPLOG_FILE_ID_DRAW_BUDGET - (ordinal - 1),
                "ordinal {ordinal} must not consume a queued id after the fault"
            );
            assert!(
                !temporary.path().join("chronicle").exists(),
                "ordinal {ordinal} must not admit the namespace"
            );
        }
    }

    #[test]
    fn preexisting_symlink_chronicle_maps_to_create_namespace_unsafe() {
        let temporary = temp();
        let root = temporary.path();
        std::os::unix::fs::symlink("elsewhere", root.join("chronicle")).unwrap();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [index as u8 + 1; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                sampled_instant: Some(instant()),
                file_ids: ids.into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(root).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                )
            },
        );
        expect_token(
            &result.unwrap_err(),
            "oplog_create_namespace_chronicle_unsafe",
        );
        assert_eq!(state.sampler_calls, 1);
        assert_eq!(
            count_event(&state, OplogCreateEvent::EntropyDraw),
            OPLOG_CREATE_ATTEMPTS
        );
        assert_eq!(
            count_event(&state, OplogCreateEvent::AdmissionBytesAccepted),
            0
        );
        assert_eq!(count_event(&state, OplogCreateEvent::Lease), 0);
        assert_eq!(count_event(&state, OplogCreateEvent::Publish), 0);
        assert!(state.attempted.is_empty());
        assert!(!root.join("elsewhere").exists());
        assert!(
            fs::symlink_metadata(root.join("chronicle"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!root.join("chronicle").join("20260901").exists());
    }

    #[test]
    fn after_chronicle_wrong_kind_day_maps_to_create_namespace_day_unsafe() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterChronicle,
            {
                let root = root.clone();
                move || {
                    fs::write(health_dir(&root).parent().unwrap(), b"not-a-directory").unwrap();
                }
            },
            {
                let root = root.clone();
                move || create(&root)
            },
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_namespace_day_unsafe");
        assert!(root.join("chronicle").is_dir());
        assert!(root.join("chronicle").join("20260901").is_file());
        assert!(!health_dir(&root).exists());
    }

    #[test]
    fn after_day_symlink_health_maps_to_create_namespace_health_unsafe() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterDay,
            {
                let root = root.clone();
                move || {
                    std::os::unix::fs::symlink("outside", health_dir(&root)).unwrap();
                }
            },
            {
                let root = root.clone();
                move || create(&root)
            },
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_namespace_health_unsafe");
        assert!(
            fs::symlink_metadata(health_dir(&root))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!root.join("outside").exists());
    }

    #[test]
    fn namespace_fault_after_chronicle_maps_to_create_io_without_leaves() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let (result, consumed) =
            run_with_oplog_namespace_fault(OplogNamespacePrimitive::AfterChronicle, {
                let root = root.clone();
                move || create(&root)
            });
        assert!(consumed);
        expect_token(&result.unwrap_err(), "oplog_create_namespace_chronicle_io");
        assert!(root.join("chronicle").is_dir());
        assert!(!root.join("chronicle").join("20260901").exists());
    }

    #[test]
    fn sampled_near_midnight_instant_controls_day_and_utc_fields() {
        let temporary = temp();
        let offset = DateTime::parse_from_rfc3339("2026-09-01T23:30:00-05:00").unwrap();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x21 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                sampled_instant: Some(offset),
                file_ids: ids.into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(temporary.path()).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                )
            },
        );
        let writer = result.unwrap();
        assert_eq!(state.sampler_calls, 1);
        assert!(writer.leaf_name().contains("20260902T043000.000000Z"));
        assert!(
            temporary
                .path()
                .join("chronicle")
                .join("20260901")
                .join("health")
                .exists()
        );
    }

    #[test]
    fn production_factory_log_and_jsonl_headers_match_hand_authored_literals() {
        const LOG_HEADER: &[u8] = b"{\"_solstone_oplog_v\":1,\"candidates\":[\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--a1b2c3d4e5f60718293a4b5c6d7e8f90--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--b0c1d2e3f405162738495a6b7c8d9e0f--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--c1d2e3f405162738495a6b7c8d9e0f10--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--d2e3f405162738495a6b7c8d9e0f1011--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--e3f405162738495a6b7c8d9e0f101112--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--f405162738495a6b7c8d9e0f10111213--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.log\"]}\n";
        const JSONL_HEADER: &[u8] = b"{\"_solstone_oplog_v\":1,\"candidates\":[\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--a1b2c3d4e5f60718293a4b5c6d7e8f90--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--b0c1d2e3f405162738495a6b7c8d9e0f--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--c1d2e3f405162738495a6b7c8d9e0f10--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--d2e3f405162738495a6b7c8d9e0f1011--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--e3f405162738495a6b7c8d9e0f101112--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--f405162738495a6b7c8d9e0f10111213--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl\"]}\n";
        let ids: Vec<[u8; 16]> = vec![
            [
                0x8f, 0x03, 0xca, 0xbe, 0xad, 0x7e, 0x44, 0x1d, 0x83, 0xf6, 0xc9, 0x2b, 0x2d, 0x89,
                0xa0, 0x21,
            ],
            [
                0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18, 0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e,
                0x8f, 0x90,
            ],
            [
                0xb0, 0xc1, 0xd2, 0xe3, 0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d,
                0x9e, 0x0f,
            ],
            [
                0xc1, 0xd2, 0xe3, 0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e,
                0x0f, 0x10,
            ],
            [
                0xd2, 0xe3, 0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f,
                0x10, 0x11,
            ],
            [
                0xe3, 0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f, 0x10,
                0x11, 0x12,
            ],
            [
                0xf4, 0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f, 0x10, 0x11,
                0x12, 0x13,
            ],
            [
                0x05, 0x16, 0x27, 0x38, 0x49, 0x5a, 0x6b, 0x7c, 0x8d, 0x9e, 0x0f, 0x10, 0x11, 0x12,
                0x13, 0x14,
            ],
        ];

        let log_root = temp();
        let (log_result, _) = with_trace(
            OplogCreateTraceState {
                sampled_instant: Some(instant()),
                file_ids: ids.clone().into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(log_root.path()).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                )
            },
        );
        let log_writer = log_result.unwrap();
        let log_path = health_dir(log_root.path()).join(log_writer.leaf_name());
        drop(log_writer);
        assert_eq!(fs::read(&log_path).unwrap(), LOG_HEADER);

        let jsonl_root = temp();
        let (jsonl_result, _) = with_trace(
            OplogCreateTraceState {
                sampled_instant: Some(instant()),
                file_ids: ids.into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(jsonl_root.path()).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Jsonl,
                )
            },
        );
        let jsonl_writer = jsonl_result.unwrap();
        let jsonl_path = health_dir(jsonl_root.path()).join(jsonl_writer.leaf_name());
        drop(jsonl_writer);
        assert_eq!(fs::read(&jsonl_path).unwrap(), JSONL_HEADER);
    }

    #[test]
    fn sampler_fault_occurs_before_any_entropy_draw() {
        let temporary = temp();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                sampler_fail: true,
                file_ids: vec![[0x11; 16]; OPLOG_CREATE_ATTEMPTS].into(),
                ..empty_trace()
            },
            || {
                create_oplog(
                    JournalRoot::open(temporary.path()).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                )
            },
        );
        expect_token(&result.unwrap_err(), "oplog_create_clock");
        assert_eq!(state.sampler_calls, 1);
        assert_eq!(count_event(&state, OplogCreateEvent::EntropyDraw), 0);
        assert!(state.file_ids.len() == OPLOG_CREATE_ATTEMPTS);
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn invalid_originals_do_not_call_the_sampler() {
        let temporary = temp();
        let (result, state) = with_trace(empty_trace(), || {
            create_oplog(
                JournalRoot::open(temporary.path()).unwrap(),
                "",
                RUN,
                OplogFormat::Log,
            )
        });
        expect_token(&result.unwrap_err(), "oplog_create_invalid_field");
        assert_eq!(state.sampler_calls, 0);
        assert_eq!(count_event(&state, OplogCreateEvent::EntropyDraw), 0);
        assert!(!temporary.path().join("chronicle").exists());
    }

    #[test]
    fn success_records_admission_sync_lease_publish_after_entropy_draws() {
        let temporary = temp();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x31 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        result.unwrap();
        assert_eq!(
            count_event(&state, OplogCreateEvent::EntropyDraw),
            OPLOG_CREATE_ATTEMPTS
        );
        assert_eq!(
            &state.events[OPLOG_CREATE_ATTEMPTS..],
            &[
                OplogCreateEvent::AdmissionBytesAccepted,
                OplogCreateEvent::SyncAll,
                OplogCreateEvent::Lease,
                OplogCreateEvent::Publish,
            ]
        );
    }

    #[test]
    fn sync_fault_writes_bytes_and_invokes_neither_lease_nor_publish() {
        let temporary = temp();
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x41 + index as u8; 16])
            .collect();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                sync_fail: true,
                file_ids: ids.into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        expect_token(&result.unwrap_err(), "oplog_create_admission_sync");
        assert_eq!(
            count_event(&state, OplogCreateEvent::AdmissionBytesAccepted),
            1
        );
        assert_eq!(count_event(&state, OplogCreateEvent::SyncAll), 0);
        assert_eq!(count_event(&state, OplogCreateEvent::Lease), 0);
        assert_eq!(count_event(&state, OplogCreateEvent::Publish), 0);
        let dir = health_dir(temporary.path());
        assert!(canonical_leaves(&dir).is_empty());
        assert_eq!(leftover_unrelated(&dir).len(), 1);
    }

    #[test]
    fn first_distinct_draw_order_binds_admission_and_publish_attempts() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let first = [0x51; 16];
        let second = [0x52; 16];
        let rest: Vec<[u8; 16]> = (0..6).map(|index| [0x53 + index as u8; 16]).collect();
        let mut ids = vec![first, first, second];
        ids.extend(rest);
        fs::write(
            health_dir(temporary.path()).join(dest_for(first)),
            b"incumbent",
        )
        .unwrap();
        let (result, state) = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        );
        let writer = result.unwrap();
        assert_eq!(writer.leaf_name(), dest_for(second));
        assert_eq!(count_event(&state, OplogCreateEvent::EntropyDraw), 9);
        assert_eq!(count_event(&state, OplogCreateEvent::Publish), 1);
        let bytes = fs::read(health_dir(temporary.path()).join(writer.leaf_name())).unwrap();
        let record = validate_oplog_admission(OsStr::new(writer.leaf_name()), &bytes).unwrap();
        assert_eq!(record.candidates()[0].file_id(), file_id_hex(&first));
        assert_eq!(record.candidates()[1].file_id(), file_id_hex(&second));
        assert_eq!(
            fs::read(health_dir(temporary.path()).join(dest_for(first))).unwrap(),
            b"incumbent"
        );
    }

    fn occupy(root: &Path, ids: impl IntoIterator<Item = [u8; 16]>) {
        let _ = health_at(root);
        let dir = health_dir(root);
        for id in ids {
            fs::write(dir.join(dest_for(id)), b"preexisting").unwrap();
        }
    }

    fn ids_from(start: u8) -> Vec<[u8; 16]> {
        (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [start + index as u8; 16])
            .collect()
    }

    fn create_with_ids(root: &Path, ids: Vec<[u8; 16]>) -> Result<OplogWriter, OplogCreateError> {
        run_with_oplog_file_ids(ids, || create(root))
    }

    fn assert_secret_free(error: &OplogCreateError) {
        let display = error.to_string();
        let debug = format!("{error:?}");
        for text in [&display, &debug] {
            assert!(!text.contains("/var/tmp"), "{text}");
            assert!(!text.contains("oplog--"), "{text}");
            assert!(!text.contains("EEXIST"), "{text}");
            assert!(!text.contains("errno"), "{text}");
        }
        assert_eq!(display, debug);
    }

    #[test]
    fn pause_points_hold_the_namespace_lock_against_a_competitor() {
        for primitive in [
            OplogCreatePrimitive::AfterStageBeforeLease,
            OplogCreatePrimitive::AfterLeaseBeforePublish,
            OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
        ] {
            let isolated = temp();
            let root = isolated.path().to_path_buf();
            run_with_oplog_create_barrier(
                primitive,
                {
                    let root = root.clone();
                    move || {
                        let second = health_at(&root);
                        let error =
                            acquire_oplog_namespace_lock_with_test_timing(&second, ZERO, ZERO)
                                .unwrap_err();
                        assert_eq!(error.to_string(), "oplog_namespace_lock_busy");
                    }
                },
                {
                    let root = root.clone();
                    move || create(&root)
                },
            )
            .unwrap();
        }

        let isolated = temp();
        let root = isolated.path().to_path_buf();
        let ids = ids_from(0x70);
        occupy(&root, ids.iter().copied().take(1));
        with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterForeignCollision,
                    Box::new({
                        let root = root.clone();
                        move || {
                            let second = health_at(&root);
                            let error =
                                acquire_oplog_namespace_lock_with_test_timing(&second, ZERO, ZERO)
                                    .unwrap_err();
                            assert_eq!(error.to_string(), "oplog_namespace_lock_busy");
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(&root),
        )
        .0
        .unwrap();

        let isolated = temp();
        let root = isolated.path().to_path_buf();
        let ids = ids_from(0x80);
        occupy(&root, ids.iter().copied());
        with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::BeforeFinalFailureClassification,
                    Box::new({
                        let root = root.clone();
                        move || {
                            let second = health_at(&root);
                            let error =
                                acquire_oplog_namespace_lock_with_test_timing(&second, ZERO, ZERO)
                                    .unwrap_err();
                            assert_eq!(error.to_string(), "oplog_namespace_lock_busy");
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(&root),
        )
        .0
        .unwrap_err();
    }

    #[test]
    fn replacing_health_after_rename_before_sync_is_final_binding() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
            {
                let root = root.clone();
                move || {
                    let dir = health_dir(&root);
                    fs::rename(&dir, root.join("health-displaced")).unwrap();
                    fs::create_dir(&dir).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_final_binding");
        assert!(error.namespace() != RetainedNamespaceState::NotEstablished);
    }

    #[test]
    fn eight_foreign_collisions_keep_one_stage_and_one_admission_record() {
        let temporary = temp();
        let ids = ids_from(0x90);
        occupy(temporary.path(), ids.iter().copied());
        let error = create_with_ids(temporary.path(), ids.clone()).unwrap_err();
        expect_token(&error, "oplog_create_destination_exhaustion");
        assert_eq!(error.collisions().len(), 8);
        assert!(matches!(
            error.namespace(),
            RetainedNamespaceState::Established(_)
        ));
        let leftover = leftover_unrelated(&health_dir(temporary.path()));
        assert_eq!(leftover.len(), 1);
        let bytes = fs::read(health_dir(temporary.path()).join(&leftover[0])).unwrap();
        assert!(bytes.starts_with(b"{\"_solstone_oplog_v\":1"));
        assert_secret_free(&error);
    }

    fn first_collision_then(
        ids: Vec<[u8; 16]>,
        root: &Path,
        extra: OplogCreateTraceState,
    ) -> OplogCreateError {
        occupy(root, [ids[0]]);
        let mut state = extra;
        state.file_ids = ids.into();
        with_trace(state, || create(root)).0.unwrap_err()
    }

    #[test]
    fn first_collision_survives_second_rename_fault() {
        let temporary = temp();
        let ids = ids_from(0xa0);
        let error = first_collision_then(
            ids,
            temporary.path(),
            OplogCreateTraceState {
                fault: Some((OplogCreatePrimitive::Rename, 2)),
                ..empty_trace()
            },
        );
        expect_token(&error, "oplog_create_rename");
        assert_eq!(error.collisions().len(), 1);
        assert_eq!(error.collisions()[0].ordinal(), 1);
    }

    #[test]
    fn first_collision_survives_second_foreign_inspect_fault() {
        let temporary = temp();
        let ids = ids_from(0xa1);
        occupy(temporary.path(), ids.iter().copied().take(2));
        let error = first_collision_then(
            ids,
            temporary.path(),
            OplogCreateTraceState {
                fault: Some((OplogCreatePrimitive::AfterForeignCollision, 2)),
                ..empty_trace()
            },
        );
        expect_token(&error, "oplog_create_destination_inspection");
        assert!(
            error
                .collisions()
                .iter()
                .any(|record| record.ordinal() == 1)
        );
    }

    #[cfg(unix)]
    #[test]
    fn first_collision_survives_directory_sync_fault() {
        let temporary = temp();
        let ids = ids_from(0xa2);
        let error = first_collision_then(
            ids,
            temporary.path(),
            OplogCreateTraceState {
                parent_sync_fail: true,
                ..empty_trace()
            },
        );
        expect_token(&error, "oplog_create_directory_sync");
        assert_eq!(error.collisions().len(), 1);
    }

    #[test]
    fn first_collision_survives_final_binding_fault() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0xa3);
        occupy(&root, [ids[0]]);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
                    Box::new({
                        let root = root.clone();
                        move || {
                            let dir = health_dir(&root);
                            fs::rename(&dir, root.join("health-displaced")).unwrap();
                            fs::create_dir(&dir).unwrap();
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_final_binding");
        assert_eq!(error.collisions().len(), 1);
    }

    #[test]
    fn first_collision_survives_second_dest_wrong_kind() {
        let temporary = temp();
        let ids = ids_from(0xa4);
        occupy(temporary.path(), [ids[0]]);
        fs::create_dir(health_dir(temporary.path()).join(dest_for(ids[1]))).unwrap();
        let error = create_with_ids(temporary.path(), ids).unwrap_err();
        expect_token(&error, "oplog_create_destination_inspection");
        assert_eq!(error.collisions().len(), 1);
        assert!(error.gaps().iter().any(|gap| {
            gap.location() == OplogEvidenceCheckpoint::DestinationInspection { ordinal: 2 }
                && gap.cause() == OplogGapCause::Changed
        }));
    }

    #[test]
    fn first_collision_survives_second_dest_alias() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0xa5);
        occupy(&root, [ids[0]]);
        let dest = dest_for(ids[1]);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterForeignCollision,
                    Box::new({
                        let dir = health_dir(&root);
                        let dest = dest.clone();
                        move || {
                            let stage = fs::read_dir(&dir)
                                .unwrap()
                                .map(|entry| entry.unwrap().file_name())
                                .find(|name| name.to_string_lossy().contains(".tmp"))
                                .expect("stage");
                            fs::hard_link(dir.join(&stage), dir.join(&dest)).unwrap();
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_alias");
        assert_eq!(error.collisions().len(), 1);
    }

    #[test]
    fn aliased_eexist_returns_own_noncanonical_and_own_landed() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0xb0);
        let dest = dest_for(ids[0]);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.clone().into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterLeaseBeforePublish,
                    Box::new({
                        let dir = health_dir(&root);
                        let dest = dest.clone();
                        move || {
                            let stage = fs::read_dir(&dir)
                                .unwrap()
                                .map(|entry| entry.unwrap().file_name())
                                .find(|name| name.to_string_lossy().contains(".tmp"))
                                .expect("stage");
                            fs::hard_link(dir.join(&stage), dir.join(&dest)).unwrap();
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_alias");
        assert!(error.observations().iter().any(|observation| matches!(
            observation,
            OplogIdentityObservation::OwnNoncanonical(_)
        )));
        assert!(
            error
                .observations()
                .iter()
                .any(|observation| matches!(observation, OplogIdentityObservation::OwnLanded(_)))
        );
        assert_eq!(canonical_leaves(&health_dir(temporary.path())).len(), 1);
        assert_eq!(fs::read(health_dir(&root).join(&dest)).unwrap()[0], b'{');
    }

    #[test]
    fn table_cells_map_stage_dest_and_nlink_to_final_identity_evidence() {
        #[derive(Clone, Copy, Debug)]
        enum Leaf {
            Own,
            Foreign,
            Absent,
        }
        let original = OplogFileIdentity::from_unix(10, 20);
        let foreign = OplogFileIdentity::from_unix(10, 21);
        let occupant = |leaf| match leaf {
            Leaf::Own => NamedOccupant::Regular {
                identity: original,
                nlink: 1,
            },
            Leaf::Foreign => NamedOccupant::Regular {
                identity: foreign,
                nlink: 1,
            },
            Leaf::Absent => NamedOccupant::Absent,
        };
        let tag = |observation: &OplogIdentityObservation| match observation {
            OplogIdentityObservation::OwnNoncanonical(_) => "SO",
            OplogIdentityObservation::OwnLanded(_) => "CO",
            OplogIdentityObservation::ForeignNoncanonical(_) => "SF",
            OplogIdentityObservation::ForeignLanded(_) => "CF",
            OplogIdentityObservation::OwnMultipleLinks { nlink, .. } => {
                if *nlink == 2 {
                    "M"
                } else {
                    "M?"
                }
            }
        };
        let gap_tag = |gap: &crate::operational_log::OplogObservationGap| match gap.cause() {
            OplogGapCause::Inconsistent => "I",
            OplogGapCause::NoVerifiedLeaf => "N",
            other => panic!("unexpected gap {other:?}"),
        };
        let expected = |stage: Leaf, dest: Leaf, nlink: u64| -> Vec<&'static str> {
            match (stage, dest, nlink) {
                (Leaf::Own, Leaf::Own, 0 | 1) => vec!["CO", "SO", "I"],
                (Leaf::Own, Leaf::Own, _) => vec!["CO", "SO", "M"],
                (Leaf::Own, Leaf::Foreign, 0) => vec!["CF", "SO", "I"],
                (Leaf::Own, Leaf::Foreign, 1) => vec!["CF", "SO"],
                (Leaf::Own, Leaf::Foreign, _) => vec!["CF", "SO", "M"],
                (Leaf::Own, Leaf::Absent, 0) => vec!["SO", "I"],
                (Leaf::Own, Leaf::Absent, 1) => vec!["SO"],
                (Leaf::Own, Leaf::Absent, _) => vec!["SO", "M"],
                (Leaf::Foreign, Leaf::Own, 0) => vec!["CO", "SF", "I"],
                (Leaf::Foreign, Leaf::Own, 1) => vec!["CO", "SF"],
                (Leaf::Foreign, Leaf::Own, _) => vec!["CO", "SF", "M"],
                (Leaf::Foreign, Leaf::Foreign, 0) => vec!["CF", "SF"],
                (Leaf::Foreign, Leaf::Foreign, 1) => vec!["CF", "SF", "N"],
                (Leaf::Foreign, Leaf::Foreign, _) => vec!["CF", "SF", "N", "M"],
                (Leaf::Foreign, Leaf::Absent, 0) => vec!["SF"],
                (Leaf::Foreign, Leaf::Absent, 1) => vec!["SF", "N"],
                (Leaf::Foreign, Leaf::Absent, _) => vec!["SF", "N", "M"],
                (Leaf::Absent, Leaf::Own, 0) => vec!["CO", "I"],
                (Leaf::Absent, Leaf::Own, 1) => vec!["CO"],
                (Leaf::Absent, Leaf::Own, _) => vec!["CO", "M"],
                (Leaf::Absent, Leaf::Foreign, 0) => vec!["CF"],
                (Leaf::Absent, Leaf::Foreign, 1) => vec!["CF", "N"],
                (Leaf::Absent, Leaf::Foreign, _) => vec!["CF", "N", "M"],
                (Leaf::Absent, Leaf::Absent, 0) => vec![],
                (Leaf::Absent, Leaf::Absent, 1) => vec!["N"],
                (Leaf::Absent, Leaf::Absent, _) => vec!["N", "M"],
            }
        };
        for stage in [Leaf::Own, Leaf::Foreign, Leaf::Absent] {
            for dest in [Leaf::Own, Leaf::Foreign, Leaf::Absent] {
                for nlink in [0_u64, 1, 2] {
                    let mut evidence = OplogCreateEvidence::not_established();
                    let stage_leaf = inspect_final_stage_leaf(
                        Ok(occupant(stage)),
                        original,
                        OsStr::new("stage"),
                        &mut evidence,
                    );
                    let dest_leaf = inspect_final_candidate_leaf(
                        Ok(occupant(dest)),
                        original,
                        OsStr::new("dest"),
                        1,
                        &mut evidence,
                    );
                    let mut own_fact_count = 0_u8;
                    if stage_leaf.own_fact {
                        own_fact_count += 1;
                    }
                    if dest_leaf.own_fact {
                        own_fact_count += 1;
                    }
                    aggregate_retained_nlink(
                        own_fact_count,
                        stage_leaf.conclusive && dest_leaf.conclusive,
                        Ok(nlink),
                        &mut evidence,
                    );
                    let snapshot = evidence.fail(OplogCreateReason::InvalidField);
                    let mut got: Vec<&str> = snapshot.observations().iter().map(tag).collect();
                    got.extend(snapshot.gaps().iter().map(gap_tag));
                    let mut want = expected(stage, dest, nlink);
                    got.sort_unstable();
                    want.sort_unstable();
                    assert_eq!(got, want, "stage={stage:?} dest={dest:?} nlink={nlink}");
                    if nlink == 2 && !want.contains(&"I") && want.contains(&"M") {
                        assert!(snapshot.observations().iter().any(|observation| matches!(
                            observation,
                            OplogIdentityObservation::OwnMultipleLinks { nlink: 2, .. }
                        )));
                    }
                }
            }
        }
    }

    #[test]
    fn replacing_an_earlier_collision_before_final_classify_is_replaced() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0xd0);
        occupy(&root, ids.iter().copied());
        let first = dest_for(ids[0]);
        // Keep this replacement inode alive while the original collision is
        // present. Writing a new file only after unlinking the original allows
        // filesystems to reuse its inode and defeats this identity-transition
        // fixture.
        let replacement = root.join("replacement");
        fs::write(&replacement, b"replaced-later").unwrap();
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::BeforeFinalFailureClassification,
                    Box::new({
                        let dir = health_dir(&root);
                        let first = first.clone();
                        let replacement = replacement.clone();
                        move || {
                            fs::remove_file(dir.join(&first)).unwrap();
                            fs::rename(&replacement, dir.join(&first)).unwrap();
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_destination_exhaustion");
        assert!(matches!(
            error.collisions()[0].occupant(),
            OplogCollisionOccupant::Replaced
        ));
    }

    #[test]
    fn removing_an_earlier_collision_before_final_classify_is_absent() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0xe0);
        occupy(&root, ids.iter().copied());
        let first = dest_for(ids[0]);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::BeforeFinalFailureClassification,
                    Box::new({
                        let dir = health_dir(&root);
                        let first = first.clone();
                        move || fs::remove_file(dir.join(&first)).unwrap()
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_destination_exhaustion");
        assert!(matches!(
            error.collisions()[0].occupant(),
            OplogCollisionOccupant::Absent
        ));
    }

    #[test]
    fn io_fault_after_land_keeps_destination_inspection_and_adds_gap() {
        let temporary = temp();
        let id = [0x12; 16];
        let error = with_trace(
            OplogCreateTraceState {
                dest_identity_io: true,
                file_ids: vec![id].into(),
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_destination_inspection");
        assert!(error.gaps().iter().any(|gap| {
            gap.location() == OplogEvidenceCheckpoint::AfterRename
                && gap.cause() == OplogGapCause::Io
        }));
        assert_secret_free(&error);
    }

    #[test]
    fn exhaustion_after_removing_stage_tmp_with_zero_nlink_is_not_no_verified_leaf() {
        // leftover_unrelated includes the `.tmp` stage (it is Unrelated, not oplog--).
        // Removing it unlinks the only remaining name; the retained handle reports nlink 0.
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0xf0);
        occupy(&root, ids.iter().copied());
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::BeforeFinalFailureClassification,
                    Box::new({
                        let dir = health_dir(&root);
                        move || {
                            for name in leftover_unrelated(&dir) {
                                fs::remove_file(dir.join(name)).unwrap();
                            }
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_destination_exhaustion");
        assert!(error.observations().iter().any(|observation| {
            matches!(observation, OplogIdentityObservation::ForeignLanded(_))
        }));
        assert!(!error.observations().iter().any(|observation| {
            matches!(
                observation,
                OplogIdentityObservation::OwnNoncanonical(_)
                    | OplogIdentityObservation::OwnLanded(_)
            )
        }));
        assert!(
            !error
                .gaps()
                .iter()
                .any(|gap| gap.cause() == OplogGapCause::NoVerifiedLeaf)
        );
    }

    #[test]
    fn unlinked_stage_with_extra_hard_link_is_no_verified_leaf() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0xf1);
        occupy(&root, ids.iter().copied());
        let stage_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![
                    (
                        OplogCreatePrimitive::AfterLeaseBeforePublish,
                        Box::new({
                            let dir = health_dir(&root);
                            let stage_slot = std::sync::Arc::clone(&stage_slot);
                            move || {
                                let stage = fs::read_dir(&dir)
                                    .unwrap()
                                    .map(|entry| entry.unwrap().file_name())
                                    .find(|name| name.to_string_lossy().contains(".tmp"))
                                    .expect("stage");
                                fs::hard_link(dir.join(&stage), dir.join("hidden-stage")).unwrap();
                                *stage_slot.lock().unwrap() =
                                    Some(stage.to_string_lossy().into_owned());
                            }
                        }),
                    ),
                    (
                        OplogCreatePrimitive::BeforeFinalFailureClassification,
                        Box::new({
                            let dir = health_dir(&root);
                            let stage_slot = std::sync::Arc::clone(&stage_slot);
                            move || {
                                let name = stage_slot.lock().unwrap().clone().expect("captured");
                                fs::remove_file(dir.join(name)).unwrap();
                            }
                        }),
                    ),
                ],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_destination_exhaustion");
        assert!(error.gaps().iter().any(|gap| {
            gap.cause() == OplogGapCause::NoVerifiedLeaf
                && gap.location() == OplogEvidenceCheckpoint::RetainedHandle
        }));
        assert!(!error.observations().iter().any(|observation| matches!(
            observation,
            OplogIdentityObservation::OwnNoncanonical(_) | OplogIdentityObservation::OwnLanded(_)
        )));
    }

    #[test]
    fn prior_own_landed_does_not_suppress_final_pass_no_verified_leaf() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = with_trace(
            OplogCreateTraceState {
                barriers: vec![
                    (
                        OplogCreatePrimitive::AfterLeaseBeforePublish,
                        Box::new({
                            let dir = health_dir(&root);
                            move || {
                                let stage = fs::read_dir(&dir)
                                    .unwrap()
                                    .map(|entry| entry.unwrap().file_name())
                                    .find(|name| name.to_string_lossy().contains(".tmp"))
                                    .expect("stage");
                                fs::hard_link(dir.join(&stage), dir.join("hidden-stage")).unwrap();
                            }
                        }),
                    ),
                    (
                        OplogCreatePrimitive::BeforeFinalFailureClassification,
                        Box::new({
                            let dir = health_dir(&root);
                            move || {
                                let dest = canonical_leaves(&dir)
                                    .into_iter()
                                    .next()
                                    .expect("landed dest");
                                fs::remove_file(dir.join(dest)).unwrap();
                            }
                        }),
                    ),
                ],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_alias");
        assert!(
            error
                .observations()
                .iter()
                .any(|observation| matches!(observation, OplogIdentityObservation::OwnLanded(_)))
        );
        assert!(error.gaps().iter().any(|gap| {
            gap.cause() == OplogGapCause::NoVerifiedLeaf
                && gap.location() == OplogEvidenceCheckpoint::RetainedHandle
        }));
    }

    #[test]
    fn extra_hard_link_after_lease_records_own_multiple_links() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterLeaseBeforePublish,
            {
                let dir = health_dir(&root);
                move || {
                    let stage = fs::read_dir(&dir)
                        .unwrap()
                        .map(|entry| entry.unwrap().file_name())
                        .find(|name| name.to_string_lossy().contains(".tmp"))
                        .expect("stage");
                    fs::hard_link(dir.join(&stage), dir.join("hidden-link")).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_alias");
        assert!(error.observations().iter().any(|observation| matches!(
            observation,
            OplogIdentityObservation::OwnMultipleLinks { nlink, .. } if *nlink > 1
        )));
    }

    #[test]
    fn post_allocation_create_path_has_no_pathname_unlink() {
        fn production(source: &str) -> &str {
            source
                .find("\n#[cfg(all(test")
                .or_else(|| source.find("\n#[cfg(test)]\n"))
                .map(|index| &source[..index])
                .unwrap_or(source)
        }
        let create = production(include_str!("create.rs"));
        let unix = production(include_str!("unix.rs"));
        let windows = production(include_str!("windows.rs"));
        for (name, source) in [("create", create), ("unix", unix)] {
            assert!(
                !source.contains("unlinkat(") && !source.contains("rollback_stage("),
                "{name} still has a pathname unlink path"
            );
        }
        assert!(
            !windows.contains("FILE_DISPOSITION") && !windows.contains("FileDispositionInfo"),
            "windows create path still has a handle-disposition delete"
        );
    }

    #[test]
    fn journal_root_symlink_ancestor_after_sync_is_final_binding() {
        let temporary = temp();
        let journal = temporary.path().join("outer/inner/journal");
        fs::create_dir_all(&journal).unwrap();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
            {
                let temporary = temporary.path().to_path_buf();
                move || {
                    let inner = temporary.join("outer/inner");
                    let moved = temporary.join("outer/inner-moved");
                    fs::rename(&inner, &moved).unwrap();
                    std::os::unix::fs::symlink(&moved, &inner).unwrap();
                }
            },
            || {
                create_oplog_with_test_timing(
                    JournalRoot::open(&journal).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                    instant(),
                    ZERO,
                    ZERO,
                )
            },
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_final_binding");
    }

    #[test]
    fn replacing_chronicle_after_rename_is_final_binding() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
            {
                let root = root.clone();
                move || {
                    let chronicle = root.join("chronicle");
                    fs::rename(&chronicle, root.join("chronicle-moved")).unwrap();
                    fs::create_dir(&chronicle).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_final_binding");
    }

    #[test]
    fn replacing_day_after_rename_is_final_binding() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
            {
                let root = root.clone();
                move || {
                    let day = health_dir(&root).parent().unwrap().to_path_buf();
                    fs::rename(&day, root.join("day-moved")).unwrap();
                    fs::create_dir(&day).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_final_binding");
    }

    #[test]
    fn recreating_stage_after_rename_is_final_binding() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let stage_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let error = with_trace(
            OplogCreateTraceState {
                barriers: vec![
                    (
                        OplogCreatePrimitive::AfterLeaseBeforePublish,
                        Box::new({
                            let dir = health_dir(&root);
                            let stage_slot = std::sync::Arc::clone(&stage_slot);
                            move || {
                                let stage = fs::read_dir(&dir)
                                    .unwrap()
                                    .map(|entry| entry.unwrap().file_name())
                                    .find(|name| name.to_string_lossy().contains(".tmp"))
                                    .expect("stage");
                                *stage_slot.lock().unwrap() =
                                    Some(stage.to_string_lossy().into_owned());
                            }
                        }),
                    ),
                    (
                        OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
                        Box::new({
                            let dir = health_dir(&root);
                            let stage_slot = std::sync::Arc::clone(&stage_slot);
                            move || {
                                let name = stage_slot.lock().unwrap().clone().expect("captured");
                                let dest = canonical_leaves(&dir)
                                    .into_iter()
                                    .next()
                                    .expect("landed dest");
                                fs::hard_link(dir.join(dest), dir.join(name)).unwrap();
                            }
                        }),
                    ),
                ],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_final_binding");
    }

    #[test]
    fn foreign_regular_recreated_at_stage_leaf_after_rename_is_final_binding() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let stage_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let dest_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let error = with_trace(
            OplogCreateTraceState {
                barriers: vec![
                    (
                        OplogCreatePrimitive::AfterLeaseBeforePublish,
                        Box::new({
                            let dir = health_dir(&root);
                            let stage_slot = std::sync::Arc::clone(&stage_slot);
                            move || {
                                let stage = fs::read_dir(&dir)
                                    .unwrap()
                                    .map(|entry| entry.unwrap().file_name())
                                    .find(|name| name.to_string_lossy().contains(".tmp"))
                                    .expect("stage");
                                *stage_slot.lock().unwrap() =
                                    Some(stage.to_string_lossy().into_owned());
                            }
                        }),
                    ),
                    (
                        OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
                        Box::new({
                            let dir = health_dir(&root);
                            let stage_slot = std::sync::Arc::clone(&stage_slot);
                            let dest_slot = std::sync::Arc::clone(&dest_slot);
                            move || {
                                let name = stage_slot.lock().unwrap().clone().expect("captured");
                                let dest = canonical_leaves(&dir)
                                    .into_iter()
                                    .next()
                                    .expect("landed dest");
                                *dest_slot.lock().unwrap() = Some(dest.clone());
                                fs::write(dir.join(name), b"foreign-stage").unwrap();
                            }
                        }),
                    ),
                ],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_final_binding");
        let dir = health_dir(&root);
        let dest = dest_slot.lock().unwrap().clone().expect("dest");
        let dest_bytes = fs::read(dir.join(&dest)).unwrap();
        assert!(dest_bytes.starts_with(b"{\"_solstone_oplog_v\":1"));
        assert_ne!(dest_bytes, b"foreign-stage");
        assert!(error.observations().iter().any(|observation| matches!(
            observation,
            OplogIdentityObservation::ForeignNoncanonical(_)
        )));
        assert!(!error.observations().iter().any(|observation| matches!(
            observation,
            OplogIdentityObservation::OwnNoncanonical(_)
        )));
        let stage = stage_slot.lock().unwrap().clone().expect("stage");
        assert_eq!(fs::read(dir.join(stage)).unwrap(), b"foreign-stage");
    }

    #[test]
    fn wrong_kind_recreated_at_stage_leaf_after_rename_is_final_binding() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let stage_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let dest_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let error = with_trace(
            OplogCreateTraceState {
                barriers: vec![
                    (
                        OplogCreatePrimitive::AfterLeaseBeforePublish,
                        Box::new({
                            let dir = health_dir(&root);
                            let stage_slot = std::sync::Arc::clone(&stage_slot);
                            move || {
                                let stage = fs::read_dir(&dir)
                                    .unwrap()
                                    .map(|entry| entry.unwrap().file_name())
                                    .find(|name| name.to_string_lossy().contains(".tmp"))
                                    .expect("stage");
                                *stage_slot.lock().unwrap() =
                                    Some(stage.to_string_lossy().into_owned());
                            }
                        }),
                    ),
                    (
                        OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
                        Box::new({
                            let dir = health_dir(&root);
                            let stage_slot = std::sync::Arc::clone(&stage_slot);
                            let dest_slot = std::sync::Arc::clone(&dest_slot);
                            move || {
                                let name = stage_slot.lock().unwrap().clone().expect("captured");
                                let dest = canonical_leaves(&dir)
                                    .into_iter()
                                    .next()
                                    .expect("landed dest");
                                *dest_slot.lock().unwrap() = Some(dest);
                                fs::create_dir(dir.join(name)).unwrap();
                            }
                        }),
                    ),
                ],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_final_binding");
        let dest = dest_slot.lock().unwrap().clone().expect("dest");
        let dest_bytes = fs::read(health_dir(&root).join(&dest)).unwrap();
        assert!(dest_bytes.starts_with(b"{\"_solstone_oplog_v\":1"));
        assert!(!error.observations().iter().any(|observation| matches!(
            observation,
            OplogIdentityObservation::ForeignNoncanonical(_)
                | OplogIdentityObservation::OwnNoncanonical(_)
        )));
    }

    #[test]
    fn new_hard_link_after_rename_is_final_binding() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterRenameBeforeDirectorySync,
            {
                let dir = health_dir(&root);
                move || {
                    let dest = canonical_leaves(&dir)
                        .into_iter()
                        .next()
                        .expect("landed dest");
                    fs::hard_link(dir.join(&dest), dir.join("late-link")).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_final_binding");
    }

    #[test]
    fn pre_health_failures_are_not_established_with_empty_evidence() {
        let temporary = temp();
        let error = create_oplog(
            JournalRoot::open(temporary.path()).unwrap(),
            "",
            RUN,
            OplogFormat::Log,
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_invalid_field");
        assert_eq!(error.namespace(), RetainedNamespaceState::NotEstablished);
        assert!(error.observations().is_empty());
        assert!(error.collisions().is_empty());
        assert!(error.gaps().is_empty());
        assert_secret_free(&error);
    }

    #[test]
    fn post_admission_failures_are_established() {
        let temporary = temp();
        let (result, _) =
            run_with_oplog_create_fault(OplogCreatePrimitive::Lease, || create(temporary.path()));
        let error = result.unwrap_err();
        expect_token(&error, "oplog_create_lease_failed");
        assert!(matches!(
            error.namespace(),
            RetainedNamespaceState::Established(_)
        ));
        assert_secret_free(&error);
    }

    #[cfg(unix)]
    #[test]
    fn directory_sync_failure_after_first_collision() {
        let temporary = temp();
        let ids = ids_from(0x11);
        occupy(temporary.path(), [ids[0]]);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                parent_sync_fail: true,
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_directory_sync");
        assert_eq!(error.collisions().len(), 1);
    }

    #[test]
    fn ancestor_revalidation_before_rename_has_no_collisions() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterLeaseBeforePublish,
            {
                let root = root.clone();
                move || {
                    let dir = health_dir(&root);
                    fs::rename(&dir, root.join("health-displaced")).unwrap();
                    std::os::unix::fs::symlink(root.join("escape"), &dir).unwrap();
                }
            },
            || create(&root),
        )
        .unwrap_err();
        expect_token(&error, "oplog_create_ancestor_revalidation");
        assert!(error.collisions().is_empty());
        assert!(error.gaps().iter().any(|gap| {
            gap.location()
                == OplogEvidenceCheckpoint::AncestorRevalidation {
                    ordinal: 1,
                    component: OplogAncestorComponent::Health,
                }
                && gap.cause() == OplogGapCause::Changed
        }));
    }

    fn ancestor_replace_at_second_attempt(
        replace: impl FnOnce(&Path) + 'static,
    ) -> OplogCreateError {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0x21);
        occupy(&root, [ids[0]]);
        let second = dest_for(ids[1]);
        let dir = health_dir(&root);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterForeignCollision,
                    Box::new({
                        let root = root.clone();
                        move || replace(&root)
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        assert!(
            !dir.join(&second).exists(),
            "candidate-2 destination must not be created after ancestor failure"
        );
        error
    }

    #[test]
    fn ancestor_revalidation_second_attempt_replacing_health_skips_candidate_two_rename() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0x22);
        occupy(&root, [ids[0]]);
        let second = dest_for(ids[1]);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterForeignCollision,
                    Box::new({
                        let root = root.clone();
                        move || {
                            let dir = health_dir(&root);
                            fs::rename(&dir, root.join("health-displaced")).unwrap();
                            fs::create_dir(&dir).unwrap();
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_ancestor_revalidation");
        assert_eq!(error.collisions().len(), 1);
        assert!(error.gaps().iter().any(|gap| {
            gap.location()
                == OplogEvidenceCheckpoint::AncestorRevalidation {
                    ordinal: 2,
                    component: OplogAncestorComponent::Health,
                }
        }));
        assert!(!health_dir(&root).join(&second).exists());
    }

    #[test]
    fn ancestor_revalidation_second_attempt_replacing_chronicle() {
        let error = ancestor_replace_at_second_attempt(|root| {
            let chronicle = root.join("chronicle");
            fs::rename(&chronicle, root.join("chronicle-moved")).unwrap();
            fs::create_dir(&chronicle).unwrap();
        });
        expect_token(&error, "oplog_create_ancestor_revalidation");
        assert_eq!(error.collisions().len(), 1);
        assert!(error.gaps().iter().any(|gap| {
            gap.location()
                == OplogEvidenceCheckpoint::AncestorRevalidation {
                    ordinal: 2,
                    component: OplogAncestorComponent::Chronicle,
                }
        }));
    }

    #[test]
    fn ancestor_revalidation_second_attempt_replacing_day() {
        let error = ancestor_replace_at_second_attempt(|root| {
            let day = health_dir(root).parent().unwrap().to_path_buf();
            fs::rename(&day, root.join("day-moved")).unwrap();
            fs::create_dir(&day).unwrap();
        });
        expect_token(&error, "oplog_create_ancestor_revalidation");
        assert_eq!(error.collisions().len(), 1);
        assert!(error.gaps().iter().any(|gap| {
            gap.location()
                == OplogEvidenceCheckpoint::AncestorRevalidation {
                    ordinal: 2,
                    component: OplogAncestorComponent::Day,
                }
        }));
    }

    #[test]
    fn ancestor_revalidation_second_attempt_replacing_root_ancestor() {
        let temporary = temp();
        let journal = temporary.path().join("outer/inner/journal");
        fs::create_dir_all(&journal).unwrap();
        let ids = ids_from(0x23);
        occupy(&journal, [ids[0]]);
        let second = dest_for(ids[1]);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterForeignCollision,
                    Box::new({
                        let temporary = temporary.path().to_path_buf();
                        move || {
                            let inner = temporary.join("outer/inner");
                            let moved = temporary.join("outer/inner-moved");
                            fs::rename(&inner, &moved).unwrap();
                            std::os::unix::fs::symlink(&moved, &inner).unwrap();
                        }
                    }),
                )],
                ..empty_trace()
            },
            || {
                create_oplog_with_test_timing(
                    JournalRoot::open(&journal).unwrap(),
                    SOURCE,
                    RUN,
                    OplogFormat::Log,
                    instant(),
                    ZERO,
                    ZERO,
                )
            },
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_ancestor_revalidation");
        assert_eq!(error.collisions().len(), 1);
        assert!(error.gaps().iter().any(|gap| {
            gap.location()
                == OplogEvidenceCheckpoint::AncestorRevalidation {
                    ordinal: 2,
                    component: OplogAncestorComponent::Root,
                }
        }));
        assert!(!health_dir(&journal).join(&second).exists());
    }

    #[test]
    fn after_successful_kernel_rename_fault_is_destination_inspection() {
        let temporary = temp();
        let (result, consumed) =
            run_with_oplog_create_fault(OplogCreatePrimitive::AfterRename, || {
                create(temporary.path())
            });
        assert!(consumed);
        expect_token(&result.unwrap_err(), "oplog_create_destination_inspection");
        assert_eq!(canonical_leaves(&health_dir(temporary.path())).len(), 1);
    }

    #[test]
    fn before_kernel_rename_fault_leaves_no_canonical_leaf() {
        let temporary = temp();
        let (result, consumed) =
            run_with_oplog_create_fault(OplogCreatePrimitive::Rename, || create(temporary.path()));
        assert!(consumed);
        expect_token(&result.unwrap_err(), "oplog_create_rename");
        assert!(canonical_leaves(&health_dir(temporary.path())).is_empty());
    }

    #[test]
    fn namespace_and_lock_mapping_uses_typed_fields_not_string_parsing() {
        let source = include_str!("create.rs");
        let production = source
            .split("\n#[cfg(all(test")
            .next()
            .or_else(|| source.split("\n#[cfg(test)]\n").next())
            .unwrap_or(source);
        assert!(
            !production.contains("strip_prefix"),
            "create.rs still parses lock/namespace errors from Display tokens"
        );
        assert!(production.contains("error.create_class()"));
        assert!(production.contains("error.create_stage()"));
    }

    #[cfg(unix)]
    #[test]
    fn eight_collision_exhaustion_closes_witness_fds_while_error_is_live() {
        let temporary = temp();
        let ids = ids_from(0x24);
        occupy(temporary.path(), ids.iter().copied());
        let dir = health_dir(temporary.path());
        let first = dir.join(dest_for(ids[0]));
        assert_eq!(open_fd_count_for(&first), 0);
        let error = create_with_ids(temporary.path(), ids.clone()).unwrap_err();
        expect_token(&error, "oplog_create_destination_exhaustion");
        assert_eq!(error.collisions().len(), 8);
        assert_eq!(open_fd_count_for(&first), 0);
        for id in &ids {
            assert_eq!(open_fd_count_for(&dir.join(dest_for(*id))), 0);
            fs::remove_file(dir.join(dest_for(*id))).unwrap();
        }
        create_with_ids(temporary.path(), ids_from(0x30)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn success_after_one_collision_closes_witness_fd() {
        let temporary = temp();
        let _ = health_at(temporary.path());
        let first_id = [0x11; 16];
        let second_id = [0x22; 16];
        let incumbent = dest_for(first_id);
        let path = health_dir(temporary.path()).join(&incumbent);
        fs::write(&path, b"incumbent").unwrap();
        assert_eq!(open_fd_count_for(&path), 0);
        let writer =
            run_with_oplog_file_ids(vec![first_id, second_id], || create(temporary.path()))
                .unwrap();
        assert_eq!(writer.leaf_name(), dest_for(second_id));
        assert_eq!(fs::read(&path).unwrap(), b"incumbent");
        assert_eq!(open_fd_count_for(&path), 0);
    }

    #[cfg(unix)]
    #[test]
    fn early_failure_after_one_collision_closes_witness_fd() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let ids = ids_from(0x25);
        occupy(&root, [ids[0]]);
        let dest_name = dest_for(ids[0]);
        let first = health_dir(&root).join(&dest_name);
        assert_eq!(open_fd_count_for(&first), 0);
        let error = with_trace(
            OplogCreateTraceState {
                file_ids: ids.into(),
                barriers: vec![(
                    OplogCreatePrimitive::AfterForeignCollision,
                    Box::new({
                        let root = root.clone();
                        move || {
                            let dir = health_dir(&root);
                            fs::rename(&dir, root.join("health-displaced")).unwrap();
                            fs::create_dir(&dir).unwrap();
                        }
                    }),
                )],
                ..empty_trace()
            },
            || create(temporary.path()),
        )
        .0
        .unwrap_err();
        expect_token(&error, "oplog_create_ancestor_revalidation");
        assert_eq!(error.collisions().len(), 1);
        let displaced = temporary.path().join("health-displaced").join(dest_name);
        assert_eq!(open_fd_count_for(&displaced), 0);
    }

    #[test]
    fn io_on_any_final_leaf_suppresses_no_verified_leaf() {
        let original = OplogFileIdentity::from_unix(3, 4);
        let mut evidence = OplogCreateEvidence::not_established();
        let stage = inspect_final_stage_leaf(
            Err(OplogGapCause::Io),
            original,
            OsStr::new("stage"),
            &mut evidence,
        );
        let dest = inspect_final_candidate_leaf(
            Ok(NamedOccupant::Absent),
            original,
            OsStr::new("dest"),
            1,
            &mut evidence,
        );
        aggregate_retained_nlink(
            u8::from(stage.own_fact) + u8::from(dest.own_fact),
            stage.conclusive && dest.conclusive,
            Ok(1),
            &mut evidence,
        );
        let snapshot = evidence.fail(OplogCreateReason::InvalidField);
        assert!(snapshot.gaps().iter().any(|gap| {
            gap.location() == OplogEvidenceCheckpoint::FinalStageInspection
                && gap.cause() == OplogGapCause::Io
        }));
        assert!(
            !snapshot
                .gaps()
                .iter()
                .any(|gap| gap.cause() == OplogGapCause::NoVerifiedLeaf)
        );
    }
}

#[cfg(test)]
mod derivation_tests {
    use chrono::DateTime;

    use super::*;

    /// DST here is two independent `FixedOffset` instants, not an IANA fold.
    ///
    /// Production only ever receives `DateTime<FixedOffset>`. This crate
    /// depends on plain `chrono`, not `chrono-tz`, so a real zone database
    /// transition is not available in tests. A caller who sampled
    /// `Local::now()` across a spring-forward hands in two nearby instants
    /// with different offsets; this checks that each call derives day-key
    /// and UTC field from that instant alone, with no persistent state.
    #[test]
    fn dst_boundary_instants_derive_independently() {
        let before = DateTime::parse_from_rfc3339("2026-03-08T01:30:00-05:00").unwrap();
        let after = DateTime::parse_from_rfc3339("2026-03-08T03:30:00-04:00").unwrap();
        let (day_before, opened_before) = derive_day_key_and_opened_field(before);
        let (day_after, opened_after) = derive_day_key_and_opened_field(after);
        assert_eq!(day_before, "20260308");
        assert_eq!(opened_before, "20260308T063000.000000Z");
        assert_eq!(day_after, "20260308");
        assert_eq!(opened_after, "20260308T073000.000000Z");
        let (day_before_again, opened_before_again) = derive_day_key_and_opened_field(before);
        assert_eq!(day_before, day_before_again);
        assert_eq!(opened_before, opened_before_again);
    }
}

#[cfg(test)]
mod write_admission_tests {
    use std::collections::VecDeque;
    use std::io::{self, ErrorKind, Write};

    use super::*;

    struct ScriptedWrite {
        plan: VecDeque<io::Result<usize>>,
        sink: Vec<u8>,
    }

    impl Write for ScriptedWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            match self.plan.pop_front() {
                Some(Ok(n)) => {
                    let n = n.min(buf.len());
                    self.sink.extend_from_slice(&buf[..n]);
                    Ok(n)
                }
                Some(Err(error)) => Err(error),
                None => {
                    self.sink.extend_from_slice(buf);
                    Ok(buf.len())
                }
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn interrupted_write_retries_until_the_header_is_accepted() {
        let header = b"{\"_solstone_oplog_v\":1}\n";
        let mut writer = ScriptedWrite {
            plan: VecDeque::from([
                Err(io::Error::from(ErrorKind::Interrupted)),
                Ok(4),
                Err(io::Error::from(ErrorKind::Interrupted)),
                Ok(header.len() - 4),
            ]),
            sink: Vec::new(),
        };
        write_admission_bytes(&mut writer, header).unwrap();
        assert_eq!(writer.sink, header);
    }

    #[test]
    fn zero_progress_write_is_io() {
        let header = b"{\"_solstone_oplog_v\":1}\n";
        let mut writer = ScriptedWrite {
            plan: VecDeque::from([Ok(0)]),
            sink: Vec::new(),
        };
        expect_io(write_admission_bytes(&mut writer, header));
        assert!(writer.sink.is_empty());
    }

    #[test]
    fn non_interrupted_write_error_is_io() {
        let header = b"{\"_solstone_oplog_v\":1}\n";
        let mut writer = ScriptedWrite {
            plan: VecDeque::from([Err(io::Error::from(ErrorKind::BrokenPipe))]),
            sink: Vec::new(),
        };
        expect_io(write_admission_bytes(&mut writer, header));
        assert!(writer.sink.is_empty());
    }

    fn expect_io(result: Result<(), std::io::Error>) {
        assert!(result.is_err());
    }
}
