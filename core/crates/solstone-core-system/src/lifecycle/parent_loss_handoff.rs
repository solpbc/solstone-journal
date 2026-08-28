// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generation-fenced hosted-service parent-loss handoff record.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, hold_lock, write_json,
};
use thiserror::Error;

use crate::process::{DescendantObservationFailure, ProcessInstance};

const FILE_MODE: u32 = 0o600;
pub const PARENT_LOSS_HANDOFF_SCHEMA_V1: u32 = 1;

/// The fixed set of services which can be hosted by the Journal supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostedServiceKind {
    Convey,
    Sense,
    Cortex,
    Spl,
}

/// One currently live service instance registered by an admitted child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentLossServiceRegistration {
    pub instance: ProcessInstance,
    pub uid: u32,
}

/// A service's terminal parent-loss shutdown evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentLossServiceWitness {
    pub parent: ProcessInstance,
    pub instance: ProcessInstance,
    pub uid: u32,
    pub listener_stopped: bool,
    pub service_runner_stopped: bool,
    pub initial_census_complete: bool,
    pub post_term_census_complete: bool,
    pub final_census_complete: bool,
    pub descendants_retired: bool,
    pub shutdown_complete: bool,
    pub descendant_failure: Option<DescendantObservationFailure>,
}

/// The only terminal decision a handoff record can publish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentLossHandoffTerminal {
    Completed,
    Unresolved {
        reason: ParentLossHandoffUnresolvedReason,
    },
}

/// A fail-closed reason the handoff cannot claim completion.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentLossHandoffUnresolvedReason {
    #[error("parent observation was unverifiable")]
    ParentUnverifiable,
    #[error("descendant observation failed: {0}")]
    Descendant(DescendantObservationFailure),
    #[error("service runner did not stop")]
    ServiceRunnerDidNotStop,
    #[error("enabled service has no registration")]
    MissingServiceRegistration,
    #[error("enabled service has no terminal witness")]
    MissingServiceWitness,
    #[error("service has more than one terminal witness")]
    DuplicateServiceWitness,
    #[error("handoff generation did not match")]
    GenerationMismatch,
    #[error("handoff record was malformed")]
    MalformedRecord,
    #[error("handoff artifact operation failed")]
    ArtifactFailure,
    #[error("handoff deadline expired")]
    DeadlineExceeded,
}

/// Outcome of a generation-compared handoff mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentLossHandoffPublishResult {
    Recorded,
    Completed,
    RejectedStale,
    RejectedTerminal,
}

/// Errors reading or replacing the handoff record.
#[derive(Debug, Error)]
pub enum ParentLossHandoffError {
    #[error("parent-loss handoff lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("parent-loss handoff I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("parent-loss handoff write failed: {0}")]
    Write(#[from] AtomicWriteError),
    #[error("parent-loss handoff unresolved: {0}")]
    Unresolved(#[from] ParentLossHandoffUnresolvedReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ParentLossHandoffRecord {
    schema: u32,
    generation: ProcessInstance,
    enabled: Vec<HostedServiceKind>,
    registrations: BTreeMap<HostedServiceKind, ParentLossServiceRegistration>,
    witnesses: BTreeMap<HostedServiceKind, ParentLossServiceWitness>,
    terminal: Option<ParentLossHandoffTerminal>,
}

/// Initialize a new supervisor generation, replacing any prior generation's record.
pub fn initialize_parent_loss_handoff(
    journal: &Path,
    generation: ProcessInstance,
    enabled: impl IntoIterator<Item = HostedServiceKind>,
) -> Result<(), ParentLossHandoffError> {
    let mut enabled: Vec<_> = enabled.into_iter().collect();
    enabled.sort();
    enabled.dedup();
    let path = record_path(journal);
    std::fs::create_dir_all(path.parent().expect("handoff record has health parent"))?;
    let _lock = handoff_lock(&path)?;
    if let Some(existing) = read_record_unlocked(&path)?
        && existing.generation == generation
    {
        if existing.enabled == enabled {
            return Ok(());
        }
        return Err(ParentLossHandoffUnresolvedReason::ArtifactFailure.into());
    }
    write_record(
        &path,
        &ParentLossHandoffRecord {
            schema: PARENT_LOSS_HANDOFF_SCHEMA_V1,
            generation,
            enabled,
            registrations: BTreeMap::new(),
            witnesses: BTreeMap::new(),
            terminal: None,
        },
    )
}

/// Register the current admitted instance for one enabled service.
///
/// Registration is intentionally overwritable while the generation remains
/// nonterminal: supervisor-directed restarts replace a crashed instance.
pub fn register_parent_loss_service(
    journal: &Path,
    generation: ProcessInstance,
    service: HostedServiceKind,
    registration: ParentLossServiceRegistration,
) -> Result<ParentLossHandoffPublishResult, ParentLossHandoffError> {
    mutate_matching_generation(journal, generation, |record| {
        if record.terminal.is_some() {
            return Ok(ParentLossHandoffPublishResult::RejectedTerminal);
        }
        if !record.enabled.contains(&service) {
            return terminal_unresolved(
                record,
                ParentLossHandoffUnresolvedReason::MissingServiceRegistration,
            );
        }
        record.registrations.insert(service, registration);
        Ok(ParentLossHandoffPublishResult::Recorded)
    })
}

/// Record one service's parent-loss witness and complete the generation when
/// every enabled service has produced eligible evidence.
pub fn record_parent_loss_service_witness(
    journal: &Path,
    generation: ProcessInstance,
    service: HostedServiceKind,
    witness: ParentLossServiceWitness,
) -> Result<ParentLossHandoffPublishResult, ParentLossHandoffError> {
    mutate_matching_generation(journal, generation, |record| {
        if record.terminal.is_some() {
            return Ok(ParentLossHandoffPublishResult::RejectedTerminal);
        }
        let Some(registration) = record.registrations.get(&service).copied() else {
            return terminal_unresolved(
                record,
                ParentLossHandoffUnresolvedReason::MissingServiceRegistration,
            );
        };
        if record.witnesses.contains_key(&service) {
            return terminal_unresolved(
                record,
                ParentLossHandoffUnresolvedReason::DuplicateServiceWitness,
            );
        }
        if let Some(reason) = witness_eligibility(generation, registration, witness) {
            return terminal_unresolved(record, reason);
        }
        record.witnesses.insert(service, witness);
        if record
            .enabled
            .iter()
            .all(|kind| record.witnesses.contains_key(kind))
        {
            record.terminal = Some(ParentLossHandoffTerminal::Completed);
            Ok(ParentLossHandoffPublishResult::Completed)
        } else {
            Ok(ParentLossHandoffPublishResult::Recorded)
        }
    })
}

/// Publish a terminal unresolved outcome when parent-loss observation itself
/// cannot support a completed witness.
pub fn record_parent_loss_service_unresolved(
    journal: &Path,
    generation: ProcessInstance,
    service: HostedServiceKind,
    reason: ParentLossHandoffUnresolvedReason,
) -> Result<ParentLossHandoffPublishResult, ParentLossHandoffError> {
    mutate_matching_generation(journal, generation, |record| {
        if record.terminal.is_some() {
            return Ok(ParentLossHandoffPublishResult::RejectedTerminal);
        }
        if !record.enabled.contains(&service) {
            return terminal_unresolved(
                record,
                ParentLossHandoffUnresolvedReason::MissingServiceRegistration,
            );
        }
        terminal_unresolved(record, reason)
    })
}

/// End a generation's bounded peer-witness wait without claiming completion.
pub fn finalize_parent_loss_handoff(
    journal: &Path,
    generation: ProcessInstance,
) -> Result<ParentLossHandoffPublishResult, ParentLossHandoffError> {
    mutate_matching_generation(journal, generation, |record| {
        if record.terminal.is_some() {
            return Ok(ParentLossHandoffPublishResult::RejectedTerminal);
        }
        let reason = if record
            .enabled
            .iter()
            .any(|kind| !record.registrations.contains_key(kind))
        {
            ParentLossHandoffUnresolvedReason::MissingServiceRegistration
        } else if record
            .enabled
            .iter()
            .any(|kind| !record.witnesses.contains_key(kind))
        {
            ParentLossHandoffUnresolvedReason::MissingServiceWitness
        } else {
            ParentLossHandoffUnresolvedReason::DeadlineExceeded
        };
        terminal_unresolved(record, reason)
    })
}

/// Read the current record for diagnostics and tests. Malformed bytes are never trusted.
pub fn read_parent_loss_handoff(
    journal: &Path,
) -> Result<Option<ParentLossHandoffTerminal>, ParentLossHandoffError> {
    Ok(read_record(journal)?.and_then(|record| record.terminal))
}

fn witness_eligibility(
    generation: ProcessInstance,
    registration: ParentLossServiceRegistration,
    witness: ParentLossServiceWitness,
) -> Option<ParentLossHandoffUnresolvedReason> {
    if witness.parent != generation || witness.instance != registration.instance {
        return Some(ParentLossHandoffUnresolvedReason::GenerationMismatch);
    }
    if witness.uid != registration.uid {
        return Some(ParentLossHandoffUnresolvedReason::Descendant(
            DescendantObservationFailure::WrongUid,
        ));
    }
    if let Some(failure) = witness.descendant_failure {
        return Some(ParentLossHandoffUnresolvedReason::Descendant(failure));
    }
    if !witness.listener_stopped || !witness.service_runner_stopped || !witness.shutdown_complete {
        return Some(ParentLossHandoffUnresolvedReason::ServiceRunnerDidNotStop);
    }
    if !witness.initial_census_complete
        || !witness.post_term_census_complete
        || !witness.final_census_complete
        || !witness.descendants_retired
    {
        return Some(ParentLossHandoffUnresolvedReason::Descendant(
            DescendantObservationFailure::CensusIncomplete,
        ));
    }
    None
}

fn terminal_unresolved(
    record: &mut ParentLossHandoffRecord,
    reason: ParentLossHandoffUnresolvedReason,
) -> Result<ParentLossHandoffPublishResult, ParentLossHandoffError> {
    record.terminal = Some(ParentLossHandoffTerminal::Unresolved {
        reason: reason.clone(),
    });
    Err(ParentLossHandoffError::Unresolved(reason))
}

fn mutate_matching_generation(
    journal: &Path,
    generation: ProcessInstance,
    mutate: impl FnOnce(
        &mut ParentLossHandoffRecord,
    ) -> Result<ParentLossHandoffPublishResult, ParentLossHandoffError>,
) -> Result<ParentLossHandoffPublishResult, ParentLossHandoffError> {
    let path = record_path(journal);
    let _lock = handoff_lock(&path)?;
    let mut record =
        read_record_unlocked(&path)?.ok_or(ParentLossHandoffUnresolvedReason::ArtifactFailure)?;
    if record.generation != generation {
        return Ok(ParentLossHandoffPublishResult::RejectedStale);
    }
    let result = mutate(&mut record);
    // An unresolved terminal decision is durable even though the caller sees a
    // typed failure; an ordinary mutation is durable before its result returns.
    write_record(&path, &record)?;
    result
}

fn record_path(journal: &Path) -> PathBuf {
    journal.join("health").join("parent-loss-handoff.json")
}

fn handoff_lock(path: &Path) -> Result<solstone_core_journal_io::FileLock, ParentLossHandoffError> {
    hold_lock(
        path,
        LockOptions {
            mode: Some(FILE_MODE),
            ..LockOptions::default()
        },
    )
    .map_err(ParentLossHandoffError::from)
}

fn read_record(journal: &Path) -> Result<Option<ParentLossHandoffRecord>, ParentLossHandoffError> {
    let path = record_path(journal);
    let _lock = handoff_lock(&path)?;
    read_record_unlocked(&path)
}

fn read_record_unlocked(
    path: &Path,
) -> Result<Option<ParentLossHandoffRecord>, ParentLossHandoffError> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let record = serde_json::from_slice(&bytes)
                .map_err(|_| ParentLossHandoffUnresolvedReason::MalformedRecord)?;
            validate_record(&record)?;
            Ok(Some(record))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn validate_record(record: &ParentLossHandoffRecord) -> Result<(), ParentLossHandoffError> {
    if record.schema != PARENT_LOSS_HANDOFF_SCHEMA_V1 {
        return Err(ParentLossHandoffUnresolvedReason::MalformedRecord.into());
    }
    if record.enabled.windows(2).any(|pair| pair[0] >= pair[1])
        || record
            .registrations
            .keys()
            .any(|service| !record.enabled.contains(service))
        || record
            .witnesses
            .keys()
            .any(|service| !record.enabled.contains(service))
    {
        return Err(ParentLossHandoffUnresolvedReason::MalformedRecord.into());
    }
    if matches!(record.terminal, Some(ParentLossHandoffTerminal::Completed))
        && record.enabled.iter().any(|service| {
            let Some(registration) = record.registrations.get(service).copied() else {
                return true;
            };
            let Some(witness) = record.witnesses.get(service).copied() else {
                return true;
            };
            witness_eligibility(record.generation, registration, witness).is_some()
        })
    {
        return Err(ParentLossHandoffUnresolvedReason::MalformedRecord.into());
    }
    Ok(())
}

fn write_record(
    path: &Path,
    record: &ParentLossHandoffRecord,
) -> Result<(), ParentLossHandoffError> {
    write_json(
        path,
        record,
        JsonWriteOptions {
            mode: Some(FILE_MODE),
            ..JsonWriteOptions::default()
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessBirth;

    fn journal() -> tempfile::TempDir {
        tempfile::tempdir_in("/var/tmp").expect("temporary journal")
    }

    fn instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    fn registration(pid: u32, birth: u64) -> ParentLossServiceRegistration {
        ParentLossServiceRegistration {
            instance: instance(pid, birth),
            uid: 501,
        }
    }

    fn witness(
        generation: ProcessInstance,
        registration: ParentLossServiceRegistration,
    ) -> ParentLossServiceWitness {
        ParentLossServiceWitness {
            parent: generation,
            instance: registration.instance,
            uid: registration.uid,
            listener_stopped: true,
            service_runner_stopped: true,
            initial_census_complete: true,
            post_term_census_complete: true,
            final_census_complete: true,
            descendants_retired: true,
            shutdown_complete: true,
            descendant_failure: None,
        }
    }

    #[test]
    fn sequential_live_registrations_replace_a_crashed_service_but_witnesses_are_one_shot() {
        let journal = journal();
        let generation = instance(10, 1);
        initialize_parent_loss_handoff(
            journal.path(),
            generation,
            [HostedServiceKind::Convey, HostedServiceKind::Sense],
        )
        .expect("initialize");
        let first = registration(20, 1);
        let replacement = registration(21, 2);
        assert_eq!(
            register_parent_loss_service(
                journal.path(),
                generation,
                HostedServiceKind::Convey,
                first
            )
            .expect("first registration"),
            ParentLossHandoffPublishResult::Recorded
        );
        register_parent_loss_service(
            journal.path(),
            generation,
            HostedServiceKind::Sense,
            registration(22, 1),
        )
        .expect("sense registration");
        assert_eq!(
            register_parent_loss_service(
                journal.path(),
                generation,
                HostedServiceKind::Convey,
                replacement,
            )
            .expect("replacement registration"),
            ParentLossHandoffPublishResult::Recorded
        );
        assert_eq!(
            record_parent_loss_service_witness(
                journal.path(),
                generation,
                HostedServiceKind::Convey,
                witness(generation, replacement),
            )
            .expect("first witness"),
            ParentLossHandoffPublishResult::Recorded
        );
        assert!(matches!(
            record_parent_loss_service_witness(
                journal.path(),
                generation,
                HostedServiceKind::Convey,
                witness(generation, replacement),
            ),
            Err(ParentLossHandoffError::Unresolved(
                ParentLossHandoffUnresolvedReason::DuplicateServiceWitness
            ))
        ));
    }

    #[test]
    fn stale_generation_is_rejected_without_mutating_the_current_record() {
        let journal = journal();
        let current = instance(10, 2);
        initialize_parent_loss_handoff(journal.path(), current, [HostedServiceKind::Convey])
            .expect("initialize");
        assert_eq!(
            register_parent_loss_service(
                journal.path(),
                instance(10, 1),
                HostedServiceKind::Convey,
                registration(20, 1),
            )
            .expect("stale rejection"),
            ParentLossHandoffPublishResult::RejectedStale
        );
        assert_eq!(
            read_parent_loss_handoff(journal.path()).expect("read"),
            None
        );
    }

    #[test]
    fn malformed_record_is_never_trusted_as_completed() {
        let journal = journal();
        let path = record_path(journal.path());
        std::fs::create_dir_all(path.parent().expect("health")).expect("health");
        std::fs::write(path, b"not-json").expect("malformed record");
        assert!(matches!(
            read_parent_loss_handoff(journal.path()),
            Err(ParentLossHandoffError::Unresolved(
                ParentLossHandoffUnresolvedReason::MalformedRecord
            ))
        ));
    }

    #[test]
    fn fresh_generation_replaces_a_leftover_generation() {
        let journal = journal();
        initialize_parent_loss_handoff(
            journal.path(),
            instance(10, 1),
            [HostedServiceKind::Convey],
        )
        .expect("old generation");
        initialize_parent_loss_handoff(journal.path(), instance(11, 2), [HostedServiceKind::Sense])
            .expect("fresh generation");
        assert_eq!(
            register_parent_loss_service(
                journal.path(),
                instance(10, 1),
                HostedServiceKind::Convey,
                registration(20, 1),
            )
            .expect("old writer rejected"),
            ParentLossHandoffPublishResult::RejectedStale
        );
    }

    #[test]
    fn all_eligible_service_witnesses_complete_the_handoff() {
        let journal = journal();
        let generation = instance(10, 1);
        initialize_parent_loss_handoff(
            journal.path(),
            generation,
            [HostedServiceKind::Convey, HostedServiceKind::Sense],
        )
        .expect("initialize");
        let convey = registration(20, 1);
        let sense = registration(21, 1);
        for (service, registration) in [
            (HostedServiceKind::Convey, convey),
            (HostedServiceKind::Sense, sense),
        ] {
            register_parent_loss_service(journal.path(), generation, service, registration)
                .expect("registration");
        }
        assert_eq!(
            record_parent_loss_service_witness(
                journal.path(),
                generation,
                HostedServiceKind::Convey,
                witness(generation, convey),
            )
            .expect("first witness"),
            ParentLossHandoffPublishResult::Recorded
        );
        assert_eq!(
            record_parent_loss_service_witness(
                journal.path(),
                generation,
                HostedServiceKind::Sense,
                witness(generation, sense),
            )
            .expect("last witness"),
            ParentLossHandoffPublishResult::Completed
        );
        assert_eq!(
            read_parent_loss_handoff(journal.path()).expect("read"),
            Some(ParentLossHandoffTerminal::Completed)
        );
    }

    #[test]
    fn failed_service_stop_is_unresolved_and_never_completed() {
        let journal = journal();
        let generation = instance(10, 1);
        initialize_parent_loss_handoff(journal.path(), generation, [HostedServiceKind::Convey])
            .expect("initialize");
        let convey = registration(20, 1);
        register_parent_loss_service(
            journal.path(),
            generation,
            HostedServiceKind::Convey,
            convey,
        )
        .expect("registration");
        let mut failed = witness(generation, convey);
        failed.service_runner_stopped = false;
        failed.shutdown_complete = false;

        assert!(matches!(
            record_parent_loss_service_witness(
                journal.path(),
                generation,
                HostedServiceKind::Convey,
                failed,
            ),
            Err(ParentLossHandoffError::Unresolved(
                ParentLossHandoffUnresolvedReason::ServiceRunnerDidNotStop
            ))
        ));
        assert_eq!(
            read_parent_loss_handoff(journal.path()).expect("read"),
            Some(ParentLossHandoffTerminal::Unresolved {
                reason: ParentLossHandoffUnresolvedReason::ServiceRunnerDidNotStop,
            })
        );
    }

    #[test]
    fn missing_stale_or_wrong_uid_witnesses_are_unresolved() {
        let journal = journal();
        let missing_generation = instance(10, 1);
        initialize_parent_loss_handoff(
            journal.path(),
            missing_generation,
            [HostedServiceKind::Convey],
        )
        .expect("initialize");
        assert!(matches!(
            finalize_parent_loss_handoff(journal.path(), missing_generation),
            Err(ParentLossHandoffError::Unresolved(
                ParentLossHandoffUnresolvedReason::MissingServiceRegistration
            ))
        ));

        let stale_generation = instance(11, 1);
        initialize_parent_loss_handoff(
            journal.path(),
            stale_generation,
            [HostedServiceKind::Convey],
        )
        .expect("fresh generation");
        let registration = registration(20, 1);
        register_parent_loss_service(
            journal.path(),
            stale_generation,
            HostedServiceKind::Convey,
            registration,
        )
        .expect("registration");
        let mut stale = witness(stale_generation, registration);
        stale.instance = instance(21, 2);
        assert!(matches!(
            record_parent_loss_service_witness(
                journal.path(),
                stale_generation,
                HostedServiceKind::Convey,
                stale,
            ),
            Err(ParentLossHandoffError::Unresolved(
                ParentLossHandoffUnresolvedReason::GenerationMismatch
            ))
        ));

        let wrong_uid_generation = instance(12, 1);
        initialize_parent_loss_handoff(
            journal.path(),
            wrong_uid_generation,
            [HostedServiceKind::Convey],
        )
        .expect("fresh generation");
        register_parent_loss_service(
            journal.path(),
            wrong_uid_generation,
            HostedServiceKind::Convey,
            registration,
        )
        .expect("registration");
        let mut wrong_uid = witness(wrong_uid_generation, registration);
        wrong_uid.uid = 502;
        assert!(matches!(
            record_parent_loss_service_witness(
                journal.path(),
                wrong_uid_generation,
                HostedServiceKind::Convey,
                wrong_uid,
            ),
            Err(ParentLossHandoffError::Unresolved(
                ParentLossHandoffUnresolvedReason::Descendant(
                    DescendantObservationFailure::WrongUid
                )
            ))
        ));
    }
}
