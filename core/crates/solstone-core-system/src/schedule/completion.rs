// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::Mutex;

use serde_json::{Map, Value, json};
#[cfg(not(unix))]
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace_checked};

use super::ScheduleError;

pub(crate) fn load_runtime_state(path: &Path) -> Result<Map<String, Value>, ScheduleError> {
    use solstone_core_journal_io::durability::{
        ArtifactId, DurableRead, read_json_durable_validated,
    };
    match read_json_durable_validated::<Value>(ArtifactId::SchedulerState, path, |val| {
        if val.is_object() {
            Ok(())
        } else {
            Err("scheduler state must be a JSON object".to_owned())
        }
    }) {
        Ok(DurableRead::Present(Value::Object(state))) => Ok(state),
        Ok(DurableRead::Present(_)) => unreachable!(),
        Ok(DurableRead::Absent | DurableRead::SetAside(_) | DurableRead::Unreadable { .. }) => {
            Ok(Map::new())
        }
        Err(error) => Err(ScheduleError::Io(error.to_string())),
    }
}

pub(crate) fn record_completion(
    lock: &Mutex<()>,
    path: &Path,
    name: &str,
    ended_at: f64,
    exit_status: &str,
    reference: &str,
) -> Result<(), ScheduleError> {
    std::fs::create_dir_all(path.parent().expect("scheduler state has a parent"))
        .map_err(io_error)?;
    record_completion_checked(lock, path, name, ended_at, exit_status, reference, &|| {
        Ok(())
    })
}

pub(crate) fn record_completion_checked(
    lock: &Mutex<()>,
    path: &Path,
    name: &str,
    ended_at: f64,
    exit_status: &str,
    reference: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), ScheduleError> {
    let parent = solstone_core_journal_io::JournalRoot::open(
        path.parent().expect("scheduler state has a parent"),
    )
    .map_err(io_error)?;
    let admitted_checkpoint = || {
        checkpoint()?;
        parent
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())
    };
    let _guard = lock.lock().expect("schedule completion lock poisoned");
    admitted_checkpoint().map_err(ScheduleError::Io)?;
    let mut state = load_runtime_state(path)?;
    let current = state
        .get(name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut current = current;
    current.insert("last_run".to_owned(), json!(ended_at));
    current.insert("last_status".to_owned(), json!(exit_status));
    current.insert("last_ref".to_owned(), json!(reference));
    state.insert(name.to_owned(), Value::Object(current));
    let bytes = serde_json::to_vec_pretty(&Value::Object(state))
        .map_err(|error| ScheduleError::Io(error.to_string()))?;
    admitted_checkpoint().map_err(ScheduleError::Io)?;
    #[cfg(unix)]
    {
        let outcome = solstone_core_journal_io::atomic_replace_bound_checked(
            &parent,
            path.file_name().expect("scheduler state has a name"),
            &bytes,
            0o600,
            &admitted_checkpoint,
        )
        .map_err(io_error)?;
        match outcome {
            solstone_core_journal_io::BoundAtomicOutcome::Published { .. } => Ok(()),
            solstone_core_journal_io::BoundAtomicOutcome::PublishedObservationUncertain {
                source,
                ..
            } => Err(io_error(format!(
                "published; observation uncertain: {source}"
            ))),
            solstone_core_journal_io::BoundAtomicOutcome::PublishedDurabilityUncertain {
                source,
                ..
            } => Err(io_error(format!(
                "published; durability uncertain: {source}"
            ))),
        }
    }
    #[cfg(not(unix))]
    atomic_replace_checked(
        path,
        &bytes,
        AtomicWriteOptions::default(),
        &admitted_checkpoint,
    )
    .map_err(io_error)
}

fn io_error(error: impl std::fmt::Display) -> ScheduleError {
    ScheduleError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn native_stage_cancellation_and_parent_replacement_preserve_both_destinations() {
        for replace_parent in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let parent = temp.path().join("owner");
            let parked = temp.path().join("parked");
            std::fs::create_dir(&parent).unwrap();
            let path = parent.join("scheduler.json");
            std::fs::write(&path, b"{}\n").unwrap();
            let before = std::fs::read(&path).unwrap();
            let saw_stage = std::cell::Cell::new(false);
            let checkpoint = || {
                if !saw_stage.get()
                    && std::fs::read_dir(&parent).unwrap().any(|entry| {
                        solstone_core_journal_io::is_publication_candidate_name(
                            &entry.unwrap().file_name(),
                        )
                    })
                {
                    saw_stage.set(true);
                    if replace_parent {
                        std::fs::rename(&parent, &parked).unwrap();
                        std::fs::create_dir(&parent).unwrap();
                        std::fs::write(&path, &before).unwrap();
                    } else {
                        return Err("declared cancellation after staging".to_owned());
                    }
                }
                Ok(())
            };
            let result = record_completion_checked(
                &Mutex::new(()),
                &path,
                "weekly",
                10.0,
                "success",
                "late-stage-ref",
                &checkpoint,
            );
            assert!(
                saw_stage.get(),
                "the actual owner must reach a native stage"
            );
            assert!(result.is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            if replace_parent {
                assert_eq!(
                    std::fs::read(parked.join("scheduler.json")).unwrap(),
                    before
                );
            }
        }
    }

    use std::fs;
    #[cfg(feature = "full-tests")]
    use std::sync::Arc;
    use std::sync::Mutex;
    #[cfg(feature = "full-tests")]
    use std::sync::atomic::{AtomicBool, Ordering};
    #[cfg(feature = "full-tests")]
    use std::time::{Duration, Instant};

    use super::record_completion_checked;
    #[cfg(feature = "full-tests")]
    use crate::queue::CallableTaskContext;

    fn state_path(root: &std::path::Path) -> std::path::PathBuf {
        let path = root.join("health/scheduler.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{}\n").unwrap();
        path
    }

    fn status_of(path: &std::path::Path) -> Option<String> {
        let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        value
            .get("weekly")
            .and_then(|entry| entry.get("last_status"))
            .and_then(|status| status.as_str())
            .map(str::to_owned)
    }

    #[test]
    fn completion_owner_publishes_last_status() {
        let directory = tempfile::tempdir().unwrap();
        let path = state_path(directory.path());
        record_completion_checked(
            &Mutex::new(()),
            &path,
            "weekly",
            10.0,
            "success",
            "ref-positive",
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(status_of(&path).as_deref(), Some("success"));
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn completion_refusal_while_the_lock_is_held_leaves_last_status_unpublished() {
        let directory = tempfile::tempdir().unwrap();
        let path = state_path(directory.path());
        let before = fs::read(&path).unwrap();
        let lock = Arc::new(Mutex::new(()));
        let held = lock.lock().unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let fail = Arc::new(AtomicBool::new(false));
        let started_flag = Arc::clone(&started);
        let fail_flag = Arc::clone(&fail);
        let state = path.clone();
        let worker_lock = Arc::clone(&lock);
        let worker = std::thread::spawn(move || {
            record_completion_checked(
                &worker_lock,
                &state,
                "weekly",
                10.0,
                "success",
                "ref",
                &|| {
                    started_flag.store(true, Ordering::Release);
                    if fail_flag.load(Ordering::Acquire) {
                        Err("cancelled".to_owned())
                    } else {
                        Ok(())
                    }
                },
            )
        });
        for _ in 0..100_000 {
            assert!(
                !started.load(Ordering::Acquire),
                "completion checkpoint ran before the lock"
            );
            std::thread::yield_now();
        }
        fail.store(true, Ordering::Release);
        drop(held);
        let error = worker.join().unwrap().unwrap_err();
        assert!(matches!(error, crate::schedule::ScheduleError::Io(_)));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(status_of(&path).is_none());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn completion_refusal_after_root_replacement_leaves_both_roots_unpublished() {
        let parent = tempfile::tempdir().unwrap();
        let journal = parent.path().join("journal");
        fs::create_dir(&journal).unwrap();
        let path = state_path(&journal);
        let before = fs::read(&path).unwrap();
        let bound = solstone_core_journal_io::JournalRoot::open(&journal).unwrap();
        let lock = Arc::new(Mutex::new(()));
        let held = lock.lock().unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let started_flag = Arc::clone(&started);
        let state = path.clone();
        let worker_lock = Arc::clone(&lock);
        let worker = std::thread::spawn(move || {
            record_completion_checked(
                &worker_lock,
                &state,
                "weekly",
                10.0,
                "success",
                "ref",
                &|| {
                    started_flag.store(true, Ordering::Release);
                    bound
                        .revalidate_canonical_binding()
                        .map_err(|error| error.to_string())
                },
            )
        });
        for _ in 0..100_000 {
            assert!(!started.load(Ordering::Acquire));
            std::thread::yield_now();
        }
        let parked = parent.path().join("parked");
        match fs::rename(&journal, &parked) {
            Ok(()) => {}
            // Windows refuses to rename a root while the completion holds a
            // handle inside it, so the replacement cannot begin and the
            // completion publishes into the only root.
            Err(error) if cfg!(windows) && error.kind() == std::io::ErrorKind::PermissionDenied => {
                drop(held);
                worker
                    .join()
                    .unwrap()
                    .expect("the completion still names the only root");
                assert!(!parked.exists());
                assert!(status_of(&path).is_some());
                return;
            }
            Err(error) => panic!("stage the replacement: {error}"),
        }
        fs::create_dir(&journal).unwrap();
        drop(held);
        let error = worker.join().unwrap().unwrap_err();
        assert!(matches!(error, crate::schedule::ScheduleError::Io(_)));
        assert_eq!(
            fs::read(parked.join("health/scheduler.json")).unwrap(),
            before
        );
        assert!(status_of(&parked.join("health/scheduler.json")).is_none());
        assert!(!journal.join("health/scheduler.json").exists());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn completion_deadline_during_the_lock_leaves_last_status_unpublished() {
        let directory = tempfile::tempdir().unwrap();
        let path = state_path(directory.path());
        let before = fs::read(&path).unwrap();
        let lock = Arc::new(Mutex::new(()));
        let held = lock.lock().unwrap();
        let deadline = Instant::now() + Duration::from_millis(40);
        let control = CallableTaskContext {
            reference: "completion-deadline-test".into(),
            cancelled: Arc::new(AtomicBool::new(false)),
            priority: Default::default(),
            deadline,
        };
        let state = path.clone();
        let worker_lock = Arc::clone(&lock);
        let worker = std::thread::spawn(move || {
            record_completion_checked(
                &worker_lock,
                &state,
                "weekly",
                10.0,
                "success",
                "ref",
                &|| {
                    if control.is_cancelled() {
                        Err("deadline".to_owned())
                    } else {
                        Ok(())
                    }
                },
            )
        });
        while Instant::now() < deadline {
            std::thread::yield_now();
        }
        drop(held);
        let error = worker.join().unwrap().unwrap_err();
        assert!(matches!(error, crate::schedule::ScheduleError::Io(_)));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(status_of(&path).is_none());
    }
}
