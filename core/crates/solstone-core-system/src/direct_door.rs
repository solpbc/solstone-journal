// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generation-fenced `health/direct-door.json` writer.
//!
//! Convey-shell and the supervisor call this module. They must not write the
//! file themselves.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, hold_lock, write_json,
};
use thiserror::Error;

const FILE_MODE: u32 = 0o600;

/// Outcome published by the Convey child after a door bind attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectDoorOutcome {
    Bound { port: u16 },
    BindFailed { port: u16 },
    Withheld { port: u16 },
}

/// Result of a generation-compared write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectDoorPublishResult {
    Published,
    RejectedStale,
}

/// Internal direct-door record. Only [`DirectDoorHealth`] is published to
/// `health/direct-door.json`; generation fencing stays in a private sidecar so
/// the readiness contract remains exactly `state` and `port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectDoorRecord {
    pub generation: u64,
    pub state: DirectDoorState,
    pub port: u16,
}

/// The complete, intentionally small sandbox-facing readiness payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DirectDoorHealth {
    state: DirectDoorState,
    port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DirectDoorGeneration {
    generation: u64,
}

/// Bound / bind-failed / withheld state stored on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectDoorState {
    Bound,
    BindFailed,
    Withheld,
}

/// Failures reading or replacing `health/direct-door.json`.
#[derive(Debug, Error)]
pub enum DirectDoorError {
    #[error("direct-door lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("direct-door I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("direct-door JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("direct-door write failed: {0}")]
    Write(#[from] AtomicWriteError),
}

/// Return the generation currently claimed by `health/direct-door.json`.
///
/// A missing file is generation 0.
pub fn peek_direct_door_generation(journal: &Path) -> Result<u64, DirectDoorError> {
    Ok(read_unlocked(journal)?.map_or(0, |record| record.generation))
}

/// Write the boot-time `withheld` record at generation 0 for `port`.
///
/// Supervisor calls this once before the first Convey spawn so the first child
/// observes a real on-disk withheld/P record rather than an inferred default.
pub fn initialize_direct_door(journal: &Path, port: u16) -> Result<(), DirectDoorError> {
    let path = record_path(journal);
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(FILE_MODE),
            ..LockOptions::default()
        },
    )?;
    write_record(
        &path,
        &DirectDoorRecord {
            generation: 0,
            state: DirectDoorState::Withheld,
            port,
        },
    )
}

/// Publish a door outcome at `generation`, rejecting a stale claim.
pub fn publish_direct_door(
    journal: &Path,
    generation: u64,
    outcome: DirectDoorOutcome,
) -> Result<DirectDoorPublishResult, DirectDoorError> {
    let path = record_path(journal);
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(FILE_MODE),
            ..LockOptions::default()
        },
    )?;
    let fallback_port = outcome_port(&outcome);
    let current = read_unlocked(journal)?.unwrap_or_else(|| synthesized_withheld(fallback_port));
    if current.generation != generation {
        return Ok(DirectDoorPublishResult::RejectedStale);
    }
    write_record(&path, &record_from_outcome(generation, outcome))?;
    Ok(DirectDoorPublishResult::Published)
}

/// Flip a matching-generation bound/bind-failed record to withheld.
///
/// Already-withheld at this generation is a no-op. A mismatched generation is
/// rejected so a replacement child's publish cannot be clobbered.
pub fn withhold_direct_door(
    journal: &Path,
    generation: u64,
    port: u16,
) -> Result<DirectDoorPublishResult, DirectDoorError> {
    let path = record_path(journal);
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(FILE_MODE),
            ..LockOptions::default()
        },
    )?;
    let current = read_unlocked(journal)?.unwrap_or_else(|| synthesized_withheld(port));
    if current.generation != generation {
        return Ok(DirectDoorPublishResult::RejectedStale);
    }
    match current.state {
        DirectDoorState::Withheld => Ok(DirectDoorPublishResult::Published),
        DirectDoorState::Bound | DirectDoorState::BindFailed => {
            write_record(
                &path,
                &DirectDoorRecord {
                    generation: generation.saturating_add(1),
                    state: DirectDoorState::Withheld,
                    port: current.port,
                },
            )?;
            Ok(DirectDoorPublishResult::Published)
        }
    }
}

fn record_path(journal: &Path) -> PathBuf {
    journal.join("health").join("direct-door.json")
}

fn generation_path(journal: &Path) -> PathBuf {
    journal.join("health").join(".direct-door-generation.json")
}

fn synthesized_withheld(port: u16) -> DirectDoorRecord {
    DirectDoorRecord {
        generation: 0,
        state: DirectDoorState::Withheld,
        port,
    }
}

fn outcome_port(outcome: &DirectDoorOutcome) -> u16 {
    match outcome {
        DirectDoorOutcome::Bound { port }
        | DirectDoorOutcome::BindFailed { port }
        | DirectDoorOutcome::Withheld { port } => *port,
    }
}

fn record_from_outcome(generation: u64, outcome: DirectDoorOutcome) -> DirectDoorRecord {
    match outcome {
        DirectDoorOutcome::Bound { port } => DirectDoorRecord {
            generation,
            state: DirectDoorState::Bound,
            port,
        },
        DirectDoorOutcome::BindFailed { port } => DirectDoorRecord {
            generation,
            state: DirectDoorState::BindFailed,
            port,
        },
        DirectDoorOutcome::Withheld { port } => DirectDoorRecord {
            generation,
            state: DirectDoorState::Withheld,
            port,
        },
    }
}

fn read_unlocked(journal: &Path) -> Result<Option<DirectDoorRecord>, DirectDoorError> {
    let path = record_path(journal);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let health: DirectDoorHealth = serde_json::from_slice(&bytes)?;
            let generation = read_generation_unlocked(journal)?;
            Ok(Some(DirectDoorRecord {
                generation,
                state: health.state,
                port: health.port,
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_generation_unlocked(journal: &Path) -> Result<u64, DirectDoorError> {
    match std::fs::read(generation_path(journal)) {
        Ok(bytes) => Ok(serde_json::from_slice::<DirectDoorGeneration>(&bytes)?.generation),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn write_record(path: &Path, record: &DirectDoorRecord) -> Result<(), DirectDoorError> {
    let journal = path
        .parent()
        .and_then(Path::parent)
        .expect("direct-door path has journal parent");
    write_json(
        path,
        &DirectDoorHealth {
            state: record.state,
            port: record.port,
        },
        JsonWriteOptions {
            mode: Some(FILE_MODE),
            ..JsonWriteOptions::default()
        },
    )?;
    write_json(
        generation_path(journal),
        &DirectDoorGeneration {
            generation: record.generation,
        },
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
    use tempfile::TempDir;

    fn journal() -> TempDir {
        TempDir::new_in("/var/tmp").expect("temp journal")
    }

    fn read_file(root: &Path) -> DirectDoorRecord {
        read_unlocked(root)
            .expect("record reads")
            .expect("record exists")
    }

    fn read_health(root: &Path) -> serde_json::Map<String, serde_json::Value> {
        serde_json::from_slice(&std::fs::read(record_path(root)).expect("health reads"))
            .expect("health parses")
    }

    #[test]
    fn peek_absent_is_generation_zero() {
        let root = journal();
        assert_eq!(peek_direct_door_generation(root.path()).unwrap(), 0);
    }

    #[test]
    fn publish_at_generation_zero_from_absent_succeeds() {
        let root = journal();
        let result =
            publish_direct_door(root.path(), 0, DirectDoorOutcome::Bound { port: 9000 }).unwrap();
        assert_eq!(result, DirectDoorPublishResult::Published);
        let record = read_file(root.path());
        assert_eq!(record.state, DirectDoorState::Bound);
        assert_eq!(record.port, 9000);
        assert_eq!(record.generation, 0);
    }

    #[test]
    fn withhold_from_bound_bumps_generation() {
        let root = journal();
        publish_direct_door(root.path(), 0, DirectDoorOutcome::Bound { port: 9000 }).unwrap();
        let result = withhold_direct_door(root.path(), 0, 9000).unwrap();
        assert_eq!(result, DirectDoorPublishResult::Published);
        let record = read_file(root.path());
        assert_eq!(record.generation, 1);
        assert_eq!(record.state, DirectDoorState::Withheld);
        assert_eq!(record.port, 9000);
    }

    #[test]
    fn stale_publish_is_rejected_and_does_not_modify_the_file() {
        let root = journal();
        publish_direct_door(root.path(), 0, DirectDoorOutcome::Bound { port: 9000 }).unwrap();
        withhold_direct_door(root.path(), 0, 9000).unwrap();
        let before = std::fs::read(record_path(root.path())).unwrap();
        let result =
            publish_direct_door(root.path(), 0, DirectDoorOutcome::Bound { port: 9001 }).unwrap();
        assert_eq!(result, DirectDoorPublishResult::RejectedStale);
        assert_eq!(std::fs::read(record_path(root.path())).unwrap(), before);
    }

    #[test]
    fn withhold_when_already_withheld_at_that_generation_is_a_noop() {
        let root = journal();
        initialize_direct_door(root.path(), 7657).unwrap();
        let before = std::fs::read(record_path(root.path())).unwrap();
        let result = withhold_direct_door(root.path(), 0, 7657).unwrap();
        assert_eq!(result, DirectDoorPublishResult::Published);
        assert_eq!(std::fs::read(record_path(root.path())).unwrap(), before);
        assert_eq!(peek_direct_door_generation(root.path()).unwrap(), 0);
    }

    #[test]
    fn withhold_with_mismatched_generation_is_rejected() {
        let root = journal();
        initialize_direct_door(root.path(), 7657).unwrap();
        publish_direct_door(root.path(), 0, DirectDoorOutcome::Bound { port: 7657 }).unwrap();
        withhold_direct_door(root.path(), 0, 7657).unwrap();
        publish_direct_door(root.path(), 1, DirectDoorOutcome::Bound { port: 7657 }).unwrap();
        let before = std::fs::read(record_path(root.path())).unwrap();
        let result = withhold_direct_door(root.path(), 0, 7657).unwrap();
        assert_eq!(result, DirectDoorPublishResult::RejectedStale);
        assert_eq!(std::fs::read(record_path(root.path())).unwrap(), before);
        assert_eq!(read_file(root.path()).state, DirectDoorState::Bound);
        assert_eq!(read_file(root.path()).generation, 1);
    }

    #[test]
    fn initialize_writes_generation_zero_withheld() {
        let root = journal();
        initialize_direct_door(root.path(), 9000).unwrap();
        let record = read_file(root.path());
        assert_eq!(record.generation, 0);
        assert_eq!(record.state, DirectDoorState::Withheld);
        assert_eq!(record.port, 9000);
        assert_eq!(peek_direct_door_generation(root.path()).unwrap(), 0);
    }

    #[test]
    fn health_payload_has_exactly_state_and_port_for_every_state() {
        let root = journal();
        initialize_direct_door(root.path(), 9000).unwrap();
        assert_eq!(
            read_health(root.path()),
            serde_json::json!({"state": "withheld", "port": 9000})
                .as_object()
                .unwrap()
                .clone()
        );
        publish_direct_door(root.path(), 0, DirectDoorOutcome::BindFailed { port: 9000 }).unwrap();
        assert_eq!(
            read_health(root.path()),
            serde_json::json!({"state": "bind_failed", "port": 9000})
                .as_object()
                .unwrap()
                .clone()
        );
        publish_direct_door(root.path(), 0, DirectDoorOutcome::Bound { port: 9000 }).unwrap();
        assert_eq!(
            read_health(root.path()),
            serde_json::json!({"state": "bound", "port": 9000})
                .as_object()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn failed_health_write_does_not_advance_the_generation_sidecar() {
        let root = journal();
        initialize_direct_door(root.path(), 9000).unwrap();
        publish_direct_door(root.path(), 0, DirectDoorOutcome::Bound { port: 9000 }).unwrap();
        std::fs::remove_file(record_path(root.path())).unwrap();
        std::fs::create_dir(record_path(root.path())).unwrap();

        assert!(withhold_direct_door(root.path(), 0, 9000).is_err());
        assert_eq!(read_generation_unlocked(root.path()).unwrap(), 0);
    }
}
