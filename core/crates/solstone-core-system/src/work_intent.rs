// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable owner intent. Source files and accepted records still decide owed work.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::durability::{ArtifactId, artifact};
use solstone_core_journal_io::{
    DailyUnitIdentity, FileLock, JsonWriteOptions, LockOptions, hold_lock,
};

/// A bounded continuation of one requested day, never a second completion ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebuildProgress {
    pub version: u32,
    pub root_binding: String,
    pub segments: Option<SegmentSeal>,
    pub daily: Option<Vec<DailyTarget>>,
    pub activity_days: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentSeal {
    pub generation: u64,
    pub raw_fingerprint: String,
    pub receipts_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DailyTarget {
    pub identity: DailyUnitIdentity,
    pub contract: String,
    /// Exact prior record digest, or absence, before any target is reset.
    pub baseline: Option<String>,
    /// Written before the target's first reset. It remains its publication token.
    pub token: Option<String>,
    pub dispatch_started: bool,
}

pub struct RebuildLease {
    #[cfg(not(windows))]
    _guard: FileLock,
    // The Windows guard is Send, not Sync. Retain it opaquely until the last
    // shared lease drops; no executor borrows or locks this private wrapper.
    #[cfg(windows)]
    _guard: std::sync::Mutex<FileLock>,
    pub id: String,
    pub day: String,
    pub resumed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentState {
    Queued,
    Claimed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerIntent {
    pub id: String,
    pub from_scratch: bool,
    pub state: IntentState,
    #[serde(default, skip_serializing_if = "is_false")]
    pub owner_stopped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<RebuildProgress>,
}

#[derive(Serialize, Deserialize)]
struct Store {
    version: u32,
    days: BTreeMap<String, OwnerIntent>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn valid_day(day: &str) -> bool {
    day.len() == 8
        && day.bytes().all(|c| c.is_ascii_digit())
        && chrono::NaiveDate::parse_from_str(day, "%Y%m%d").is_ok()
}

pub fn read(journal: &Path) -> Result<BTreeMap<String, OwnerIntent>, String> {
    let bytes = match std::fs::read(journal.join(artifact(ArtifactId::WorkIntent).path)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.to_string()),
    };
    let store: Store = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if store.version != 1
        || store.days.iter().any(|(day, intent)| {
            !valid_day(day)
                || intent.id.len() != 32
                || !intent.id.bytes().all(|c| c.is_ascii_hexdigit())
                || intent.rebuild.as_ref().is_some_and(|progress| {
                    progress.version != 1
                        || progress.root_binding.is_empty()
                        || progress.daily.as_ref().is_some_and(|targets| {
                            targets.iter().any(|target| {
                                !target.identity.day.is_empty() && target.identity.day != *day
                            }) || targets.iter().enumerate().any(|(i, target)| {
                                targets[..i]
                                    .iter()
                                    .any(|prior| prior.identity == target.identity)
                            })
                        })
                })
        })
    {
        return Err("unrecognized work intent".into());
    }
    Ok(store.days)
}

fn mutate<T>(
    journal: &Path,
    update: impl FnOnce(&mut BTreeMap<String, OwnerIntent>) -> Result<T, String>,
) -> Result<T, String> {
    mutate_checked(journal, &|| Ok(()), update)
}

fn mutate_checked<T>(
    journal: &Path,
    checkpoint: &dyn Fn() -> Result<(), String>,
    update: impl FnOnce(&mut BTreeMap<String, OwnerIntent>) -> Result<T, String>,
) -> Result<T, String> {
    checkpoint()?;
    let path = journal.join(artifact(ArtifactId::WorkIntent).path);
    std::fs::create_dir_all(path.parent().expect("health parent"))
        .map_err(|error| error.to_string())?;
    let _lock = hold_lock(path.with_extension("lock"), LockOptions::default())
        .map_err(|error| error.to_string())?;
    let mut days = read(journal)?;
    checkpoint()?;
    let result = update(&mut days)?;
    checkpoint()?;
    let store = Store { version: 1, days };
    #[cfg(unix)]
    {
        let parent = std::fs::File::open(path.parent().expect("health parent"))
            .map_err(|e| e.to_string())?;
        let bytes = solstone_core_journal_io::atomic::serialize_json_bytes(
            &store,
            JsonWriteOptions::default(),
        )
        .map_err(|e| e.to_string())?;
        match solstone_core_journal_io::atomic::atomic_replace_bound_checked(
            &parent,
            path.file_name().expect("work intent name"),
            &bytes,
            0o600,
            checkpoint,
        )
        .map_err(|e| e.to_string())?
        {
            solstone_core_journal_io::atomic::BoundAtomicOutcome::Published { .. } => {}
            other => return Err(format!("work intent publication is uncertain: {other:?}")),
        }
    }
    #[cfg(windows)]
    solstone_core_journal_io::write_json_checked(
        path,
        &store,
        JsonWriteOptions::default(),
        checkpoint,
    )
    .map_err(|e| e.to_string())?;
    Ok(result)
}

pub fn register(journal: &Path, day: &str, from_scratch: bool) -> Result<OwnerIntent, String> {
    if !valid_day(day) {
        return Err("invalid work day".into());
    }
    // A live executor owns publication until all its in-process work has joined.
    let _execution = execution_lease(journal, day)?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| error.to_string())?;
    let intent = OwnerIntent {
        id: random.iter().map(|byte| format!("{byte:02x}")).collect(),
        from_scratch,
        state: IntentState::Queued,
        owner_stopped: false,
        rebuild: None,
    };
    mutate(journal, |days| {
        days.insert(day.to_owned(), intent.clone());
        Ok(intent)
    })
}

fn execution_path(journal: &Path, day: &str) -> Result<PathBuf, String> {
    if !valid_day(day) {
        return Err("invalid work day".into());
    }
    Ok(journal.join("health").join(format!("rebuild-{day}.lock")))
}

fn execution_lease(journal: &Path, day: &str) -> Result<FileLock, String> {
    hold_lock(
        execution_path(journal, day)?,
        LockOptions {
            timeout: Duration::ZERO,
            ..LockOptions::default()
        },
    )
    .map_err(|_| "requested day is owned by another executor".into())
}

/// Discovery is advisory. Admission must retain the execution lease itself.
pub fn can_resume(journal: &Path, day: &str, intent: &OwnerIntent) -> Result<bool, String> {
    Ok(intent.state == IntentState::Claimed
        && intent.from_scratch
        && !intent.owner_stopped
        && intent
            .rebuild
            .as_ref()
            .is_some_and(|p| p.segments.is_some())
        && !solstone_core_journal_io::lock_is_held(execution_path(journal, day)?)
            .map_err(|e| e.to_string())?)
}

/// Only a host with joined in-process execution and durable owner Stop uses this API.
pub fn begin_rebuild(
    journal: &Path,
    day: &str,
    id: &str,
    binding: &str,
) -> Result<RebuildLease, String> {
    let guard = execution_lease(journal, day)?;
    let resumed = mutate(journal, |days| {
        let intent = days
            .get_mut(day)
            .filter(|i| i.id == id && i.from_scratch && !i.owner_stopped)
            .ok_or("requested day was superseded")?;
        match intent.state {
            IntentState::Queued if intent.rebuild.is_none() => {
                intent.state = IntentState::Claimed;
                intent.rebuild = Some(RebuildProgress {
                    version: 1,
                    root_binding: binding.into(),
                    segments: None,
                    daily: None,
                    activity_days: BTreeSet::from([day.to_owned()]),
                });
                Ok(false)
            }
            IntentState::Claimed
                if intent
                    .rebuild
                    .as_ref()
                    .is_some_and(|p| p.root_binding == binding && p.segments.is_some()) =>
            {
                Ok(true)
            }
            _ => Err("requested day needs owner review".into()),
        }
    })?;
    Ok(RebuildLease {
        #[cfg(not(windows))]
        _guard: guard,
        #[cfg(windows)]
        _guard: std::sync::Mutex::new(guard),
        id: id.into(),
        day: day.into(),
        resumed,
    })
}

pub fn update_rebuild<T>(
    journal: &Path,
    day: &str,
    id: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
    update: impl FnOnce(&mut RebuildProgress) -> Result<T, String>,
) -> Result<T, String> {
    mutate_checked(journal, checkpoint, |days| {
        let progress = days
            .get_mut(day)
            .filter(|i| i.id == id && i.state == IntentState::Claimed && !i.owner_stopped)
            .and_then(|i| i.rebuild.as_mut())
            .ok_or("requested day is stopped or superseded")?;
        update(progress)
    })
}

/// Called after the checked drain, before acknowledging an explicit owner Stop.
pub fn stop_rebuilds(journal: &Path) -> Result<(), String> {
    mutate(journal, |days| {
        for intent in days.values_mut() {
            intent.owner_stopped = true;
            // Older readers ignore the new provenance. Their existing Failed
            // hold also prevents a queued reset or a claimed finisher on rollback.
            if intent.from_scratch {
                intent.state = IntentState::Failed;
            }
        }
        Ok(())
    })
}

/// No destructive work may start unless this write succeeds. A claimed intent
/// remains held after a crash; it cannot be automatically claimed again.
pub fn claim_from_scratch(journal: &Path, day: &str, id: &str) -> Result<(), String> {
    mutate(journal, |days| {
        let intent = days
            .get_mut(day)
            .filter(|intent| {
                intent.id == id
                    && intent.from_scratch
                    && intent.state == IntentState::Queued
                    && !intent.owner_stopped
            })
            .ok_or("destructive request is no longer waiting")?;
        intent.state = IntentState::Claimed;
        Ok(())
    })
}

pub fn finish_from_scratch(
    journal: &Path,
    day: &str,
    id: &str,
    success: bool,
) -> Result<(), String> {
    finish_from_scratch_checked(journal, day, id, success, &|| Ok(()))
}

pub fn finish_from_scratch_checked(
    journal: &Path,
    day: &str,
    id: &str,
    success: bool,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    mutate_checked(journal, checkpoint, |days| {
        if let Some(intent) = days.get_mut(day).filter(|intent| {
            intent.id == id
                && intent.from_scratch
                && intent.state == IntentState::Claimed
                && !intent.owner_stopped
        }) {
            if success {
                days.remove(day);
            } else {
                intent.state = IntentState::Failed;
            }
        }
        Ok(())
    })
}

/// Called only after the discovery reader has verified current source coverage.
pub fn retire_current(journal: &Path, day: &str, id: &str) -> Result<(), String> {
    retire_current_checked(journal, day, id, &|| Ok(()))
}

pub fn retire_current_checked(
    journal: &Path,
    day: &str,
    id: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    mutate_checked(journal, checkpoint, |days| {
        if days
            .get(day)
            .is_some_and(|intent| intent.id == id && !intent.from_scratch)
        {
            days.remove(day);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuild_lease_can_be_retained_by_shared_workers() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RebuildLease>();
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn final_worker_lease_excludes_recovery_until_worker_release() {
        use std::sync::{Arc, mpsc};

        let root = tempfile::tempdir().unwrap();
        let day = "20260101";
        let request = register(root.path(), day, true).unwrap();
        let lease = Arc::new(begin_rebuild(root.path(), day, &request.id, "root").unwrap());
        update_rebuild(root.path(), day, &request.id, &|| Ok(()), |progress| {
            progress.segments = Some(SegmentSeal {
                generation: 1,
                raw_fingerprint: "input".into(),
                receipts_digest: "proof".into(),
            });
            Ok(())
        })
        .unwrap();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let worker_lease = Arc::clone(&lease);
        let worker = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            drop(worker_lease);
        });
        ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(lease);
        let blocked = begin_rebuild(root.path(), day, &request.id, "root");
        // Join even if admission is broken, so assertion failure cannot leave
        // the worker holding a lease beyond this test's bounded handshake.
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(matches!(
            blocked,
            Err(error) if error == "requested day is owned by another executor"
        ));
        let resumed = begin_rebuild(root.path(), day, &request.id, "root").unwrap();
        assert!(resumed.resumed);
    }

    #[test]
    fn recovery_requires_provenance_completed_segment_phase_and_released_executor() {
        let root = tempfile::tempdir().unwrap();
        let request = register(root.path(), "20260101", true).unwrap();
        let lease = begin_rebuild(root.path(), "20260101", &request.id, "root-a").unwrap();
        assert!(!lease.resumed);
        assert!(begin_rebuild(root.path(), "20260101", &request.id, "root-a").is_err());
        assert!(register(root.path(), "20260101", true).is_err());
        assert!(
            !can_resume(
                root.path(),
                "20260101",
                &read(root.path()).unwrap()["20260101"]
            )
            .unwrap()
        );
        drop(lease);
        // No seal: absence of an executor or Stop record alone proves nothing.
        assert!(begin_rebuild(root.path(), "20260101", &request.id, "root-a").is_err());
        update_rebuild(root.path(), "20260101", &request.id, &|| Ok(()), |p| {
            p.segments = Some(SegmentSeal {
                generation: 1,
                raw_fingerprint: "input".into(),
                receipts_digest: "proof".into(),
            });
            Ok(())
        })
        .unwrap();
        assert!(
            can_resume(
                root.path(),
                "20260101",
                &read(root.path()).unwrap()["20260101"]
            )
            .unwrap()
        );
        assert!(begin_rebuild(root.path(), "20260101", &request.id, "root-b").is_err());
        let continued = begin_rebuild(root.path(), "20260101", &request.id, "root-a").unwrap();
        assert!(continued.resumed);
        assert!(
            !can_resume(
                root.path(),
                "20260101",
                &read(root.path()).unwrap()["20260101"]
            )
            .unwrap()
        );
    }

    #[test]
    fn two_contenders_cannot_both_admit_a_recovered_request() {
        let root = tempfile::tempdir().unwrap();
        let day = "20260101";
        let request = register(root.path(), day, true).unwrap();
        let lease = begin_rebuild(root.path(), day, &request.id, "root").unwrap();
        update_rebuild(root.path(), day, &request.id, &|| Ok(()), |p| {
            p.segments = Some(SegmentSeal {
                generation: 0,
                raw_fingerprint: "input".into(),
                receipts_digest: "proof".into(),
            });
            Ok(())
        })
        .unwrap();
        drop(lease);
        let start = std::sync::Barrier::new(3);
        let admitted = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let handles = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        let result = begin_rebuild(root.path(), day, &request.id, "root");
                        admitted.wait(); // Keep the winning lease until both attempts return.
                        result.is_ok()
                    })
                })
                .collect::<Vec<_>>();
            start.wait();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|admitted| **admitted).count(), 1);
    }

    #[test]
    fn owner_stop_survives_reload_and_does_not_release_legacy_or_queued_requests() {
        let root = tempfile::tempdir().unwrap();
        let queued = register(root.path(), "20260101", true).unwrap();
        let claimed = register(root.path(), "20260102", true).unwrap();
        let lease = begin_rebuild(root.path(), "20260102", &claimed.id, "root").unwrap();
        stop_rebuilds(root.path()).unwrap();
        assert!(
            read(root.path())
                .unwrap()
                .values()
                .all(|intent| intent.owner_stopped)
        );
        let wire: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("health/work-intent.json")).unwrap(),
        )
        .unwrap();
        for day in ["20260101", "20260102"] {
            assert_eq!(
                wire["days"][day]["state"], "failed",
                "legacy readers must neither start a queued reset nor finish a stopped claim"
            );
        }
        drop(lease);
        assert!(claim_from_scratch(root.path(), "20260101", &queued.id).is_err());
        assert!(begin_rebuild(root.path(), "20260102", &claimed.id, "root").is_err());
        finish_from_scratch(root.path(), "20260102", &claimed.id, true).unwrap();
        assert!(read(root.path()).unwrap().contains_key("20260102"));
        let replacement = register(root.path(), "20260102", true).unwrap();
        assert!(!replacement.owner_stopped);
        assert!(
            update_rebuild(root.path(), "20260102", &claimed.id, &|| Ok(()), |_| Ok(())).is_err()
        );
        finish_from_scratch(root.path(), "20260102", &claimed.id, true).unwrap();
        assert_eq!(read(root.path()).unwrap()["20260102"], replacement);
    }

    #[test]
    fn stop_refuses_unreadable_authority_instead_of_acknowledging_it() {
        let root = tempfile::tempdir().unwrap();
        register(root.path(), "20260101", true).unwrap();
        std::fs::write(root.path().join("health/work-intent.json"), "{").unwrap();
        assert!(stop_rebuilds(root.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join("health/work-intent.json")).unwrap(),
            "{"
        );
    }

    #[cfg(all(unix, feature = "test-hooks"))]
    #[test]
    fn stop_durability_uncertainty_is_not_an_acknowledgement() {
        use solstone_core_journal_io::atomic::{
            BoundPublicationPrimitive, run_with_bound_publication_fault,
        };
        let root = tempfile::tempdir().unwrap();
        register(root.path(), "20260101", true).unwrap();
        let (result, consumed) =
            run_with_bound_publication_fault(BoundPublicationPrimitive::ParentSync, 1, 5, || {
                stop_rebuilds(root.path())
            });
        assert!(consumed);
        assert!(result.is_err());
        assert!(read(root.path()).unwrap()["20260101"].owner_stopped);
        stop_rebuilds(root.path()).unwrap();
        assert!(read(root.path()).unwrap()["20260101"].owner_stopped);
    }

    #[test]
    fn claimed_attempt_never_repeats_and_old_terminal_cannot_remove_new_intent() {
        let root = tempfile::tempdir().unwrap();
        let first = register(root.path(), "20260101", true).unwrap();
        claim_from_scratch(root.path(), "20260101", &first.id).unwrap();
        assert!(claim_from_scratch(root.path(), "20260101", &first.id).is_err());
        assert_eq!(
            read(root.path()).unwrap()["20260101"].state,
            IntentState::Claimed
        );
        let second = register(root.path(), "20260101", true).unwrap();
        finish_from_scratch(root.path(), "20260101", &first.id, true).unwrap();
        assert_eq!(read(root.path()).unwrap()["20260101"], second);
        claim_from_scratch(root.path(), "20260101", &second.id).unwrap();
        finish_from_scratch(root.path(), "20260101", &second.id, false).unwrap();
        assert!(claim_from_scratch(root.path(), "20260101", &second.id).is_err());
        assert_eq!(
            read(root.path()).unwrap()["20260101"].state,
            IntentState::Failed
        );
    }

    #[test]
    fn interrupted_from_scratch_finish_stays_claimed() {
        let root = tempfile::tempdir().unwrap();
        let intent = register(root.path(), "20260101", true).unwrap();
        claim_from_scratch(root.path(), "20260101", &intent.id).unwrap();
        let error = finish_from_scratch_checked(root.path(), "20260101", &intent.id, true, &|| {
            Err("stopped".into())
        })
        .unwrap_err();
        assert_eq!(error, "stopped");
        assert_eq!(
            read(root.path()).unwrap()["20260101"].state,
            IntentState::Claimed
        );
        let error =
            finish_from_scratch_checked(root.path(), "20260101", &intent.id, false, &|| {
                Err("stopped".into())
            })
            .unwrap_err();
        assert_eq!(error, "stopped");
        assert_eq!(
            read(root.path()).unwrap()["20260101"].state,
            IntentState::Claimed
        );
    }

    #[test]
    fn malformed_intent_is_preserved_and_cannot_authorize_work() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("health")).unwrap();
        let path = root.path().join("health/work-intent.json");
        std::fs::write(&path, "{").unwrap();
        assert!(register(root.path(), "20260101", true).is_err());
        assert!(claim_from_scratch(root.path(), "20260101", "request").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{");
    }
}
