// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-owned local-model install threads, retained through native Stop.

use serde_json::{Value, json};
use solstone_core_journal_io::{JournalRoot, whole_journal_restore::begin_target_use_bound};
use solstone_core_local::install::{lease, mobile, status};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

static INSTALLERS: Mutex<BTreeMap<PathBuf, Arc<Entry>>> = Mutex::new(BTreeMap::new());
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);

struct Entry {
    closed: AtomicBool,
    worker: Mutex<Option<Worker>>,
}
struct Worker {
    _root: Arc<JournalRoot>,
    cancelled: Arc<AtomicBool>,
    attempt: Arc<Mutex<Option<String>>>,
    task: Option<JoinHandle<()>>,
    failure: Option<String>,
}

fn entry(journal: &Path) -> Result<Option<Arc<Entry>>, String> {
    Ok(INSTALLERS
        .lock()
        .map_err(|_| "installer registry unavailable")?
        .get(journal)
        .cloned())
}

pub(super) fn prepare(journal: &Path) -> Result<(), String> {
    let entry = {
        let mut entries = INSTALLERS
            .lock()
            .map_err(|_| "installer registry unavailable")?;
        entries
            .entry(journal.to_owned())
            .or_insert_with(|| {
                Arc::new(Entry {
                    closed: AtomicBool::new(true),
                    worker: Mutex::new(None),
                })
            })
            .clone()
    };
    join(&entry, Instant::now())?;
    entry.closed.store(false, Ordering::Release);
    Ok(())
}

pub(super) fn request_stop(journal: &Path) {
    match entry(journal) {
        Ok(Some(entry)) => entry.closed.store(true, Ordering::Release),
        Ok(None) => {}
        Err(error) => log::error!("could not request local installer stop: {error}"),
    }
}

pub(super) fn stop(journal: &Path) -> Result<(), String> {
    let Some(entry) = entry(journal)? else {
        return Ok(());
    };
    entry.closed.store(true, Ordering::Release);
    join(&entry, Instant::now() + JOIN_TIMEOUT)
}

fn join(entry: &Entry, deadline: Instant) -> Result<(), String> {
    let mut slot = entry
        .worker
        .lock()
        .map_err(|_| "installer owner unavailable")?;
    let Some(worker) = slot.as_mut() else {
        return Ok(());
    };
    if let Some(error) = &worker.failure {
        return Err(error.clone());
    }
    if let Some(task) = worker.task.as_ref() {
        while !task.is_finished() {
            if Instant::now() >= deadline {
                return Err("local installer is still stopping".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    if worker.task.take().is_some_and(|task| task.join().is_err()) {
        worker.failure = Some("local installer worker panicked".into());
        return Err("local installer worker panicked".into());
    }
    *slot = None;
    Ok(())
}

pub(super) fn start(root: Arc<JournalRoot>, model: &str, automatic: bool) -> Result<Value, String> {
    let _admission = super::ADMISSION
        .lock()
        .map_err(|_| "installer admission unavailable")?;
    root.revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    let journal = root.canonical_path().to_owned();
    let entry = entry(&journal)?.ok_or("local installer has not been prepared")?;
    if entry.closed.load(Ordering::Acquire) {
        return Err("local installer is stopped".into());
    }
    let current = status::read_status(&journal, "local").map_err(|error| error.to_string())?;
    if lease::is_held(&journal, "local").map_err(|error| error.to_string())? {
        return if status::is_in_flight(&current.install_state) {
            Ok(solstone_core_thinking::local::bootstrap_status(
                &journal, model,
            ))
        } else {
            Err("installer lease is busy".into())
        };
    }
    if automatic && current.error_code.as_deref() == Some("install_cancelled") {
        return Err("install_cancelled".into());
    }
    join(&entry, Instant::now())?;
    let use_guard = begin_target_use_bound(root.clone()).map_err(|error| error.to_string())?;
    let lease = lease::acquire(&journal, "local")
        .map_err(|error| error.to_string())?
        .ok_or("installer lease is busy")?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let attempt = Arc::new(Mutex::new(None));
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut slot = entry
        .worker
        .lock()
        .map_err(|_| "installer owner unavailable")?;
    if entry.closed.load(Ordering::Acquire) {
        return Err("local installer is stopped".into());
    }
    let worker_entry = entry.clone();
    let worker_cancel = cancelled.clone();
    let worker_attempt = attempt.clone();
    let worker_journal = journal.clone();
    let worker_model = model.to_owned();
    let task = thread::Builder::new()
        .name("local-model-install".into())
        .spawn(move || {
            let _use_guard = use_guard;
            let cancel = || {
                worker_cancel.load(Ordering::Acquire) || worker_entry.closed.load(Ordering::Acquire)
            };
            let admitted = AtomicBool::new(false);
            let result = mobile::install_with_lease_admitted(
                &worker_journal,
                &worker_model,
                lease,
                json!({"kind":"app-owned-installer"}),
                &cancel,
                |state| {
                    if let Ok(mut attempt) = worker_attempt.lock() {
                        *attempt = state.attempt_id.clone();
                    }
                    admitted.store(true, Ordering::Release);
                    let _ = sender.send(Ok(()));
                },
            );
            if let Err(error) = result {
                let message = error
                    .envelope
                    .error
                    .map_or("model install failed".into(), |error| error.message);
                log::warn!("local model installation ended: {message}");
                if !admitted.load(Ordering::Acquire) {
                    let _ = sender.send(Err(message));
                }
            }
        })
        .map_err(|error| error.to_string())?;
    *slot = Some(Worker {
        _root: root,
        cancelled,
        attempt,
        task: Some(task),
        failure: None,
    });
    drop(slot);
    receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "installer admission unavailable")??;
    Ok(solstone_core_thinking::local::bootstrap_status(
        &journal, model,
    ))
}

pub(super) fn cancel(journal: &Path, attempt: &str) -> Result<status::InstallStatus, String> {
    let _admission = super::ADMISSION
        .lock()
        .map_err(|_| "installer admission unavailable")?;
    let current = status::read_status(journal, "local").map_err(|error| error.to_string())?;
    if current.attempt_id.as_deref() != Some(attempt) {
        return Err("install attempt changed".into());
    }
    if !status::is_in_flight(&current.install_state) {
        return Ok(current);
    }
    let Some(entry) = entry(journal)? else {
        return Err("installer owner unavailable".into());
    };
    {
        let slot = entry
            .worker
            .lock()
            .map_err(|_| "installer owner unavailable")?;
        if let Some(worker) = slot.as_ref() {
            if worker
                .attempt
                .lock()
                .map_err(|_| "installer attempt unavailable")?
                .as_deref()
                != Some(attempt)
            {
                return Err("install attempt changed".into());
            }
            worker.cancelled.store(true, Ordering::Release);
        } else if lease::is_held(journal, "local").map_err(|error| error.to_string())? {
            return Err("installer owner unavailable".into());
        }
    }
    join(&entry, Instant::now() + JOIN_TIMEOUT)?;
    status::read_observed_status(journal, "local").map_err(|error| error.to_string())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn root() -> (tempfile::TempDir, Arc<JournalRoot>) {
        let directory = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
        (directory, root)
    }

    #[test]
    fn stop_closes_admission_and_pending_join_retains_the_actual_root_and_worker() {
        let (_directory, root) = root();
        let path = root.canonical_path().to_owned();
        prepare(&path).unwrap();
        let entry = entry(&path).unwrap().unwrap();
        let weak = Arc::downgrade(&root);
        let guard = begin_target_use_bound(root.clone()).unwrap();
        let (observed_tx, observed_rx) = mpsc::channel();
        let (retire_tx, retire_rx) = mpsc::channel();
        let worker_entry = entry.clone();
        let task = thread::spawn(move || {
            let _guard = guard;
            while !worker_entry.closed.load(Ordering::Acquire) {
                thread::yield_now();
            }
            observed_tx.send(()).unwrap();
            retire_rx.recv().unwrap();
        });
        *entry.worker.lock().unwrap() = Some(Worker {
            _root: root,
            cancelled: Arc::new(AtomicBool::new(false)),
            attempt: Arc::new(Mutex::new(Some("attempt".into()))),
            task: Some(task),
            failure: None,
        });
        request_stop(&path);
        observed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(join(&entry, Instant::now()).is_err());
        assert!(
            prepare(&path).is_err(),
            "a new Start cannot cover over an unjoined worker"
        );
        assert!(entry.closed.load(Ordering::Acquire));
        assert!(
            entry
                .worker
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .task
                .is_some()
        );
        assert!(weak.upgrade().is_some());
        retire_tx.send(()).unwrap();
        stop(&path).unwrap();
        assert!(entry.worker.lock().unwrap().is_none());
        assert!(
            weak.upgrade().is_none(),
            "only actual joined retirement releases the retained root"
        );
        prepare(&path).unwrap();
        assert!(!entry.closed.load(Ordering::Acquire));
        INSTALLERS.lock().unwrap().remove(&path);
    }

    #[test]
    fn a_panicked_worker_stays_closed_and_retains_its_root_after_join_failure() {
        let (_directory, root) = root();
        let path = root.canonical_path().to_owned();
        prepare(&path).unwrap();
        let entry = entry(&path).unwrap().unwrap();
        let weak = Arc::downgrade(&root);
        *entry.worker.lock().unwrap() = Some(Worker {
            _root: root,
            cancelled: Arc::new(AtomicBool::new(false)),
            attempt: Arc::new(Mutex::new(None)),
            task: Some(thread::spawn(|| panic!("intentional installer panic"))),
            failure: None,
        });
        assert!(stop(&path).unwrap_err().contains("panicked"));
        assert!(prepare(&path).is_err());
        assert!(entry.closed.load(Ordering::Acquire));
        assert!(weak.upgrade().is_some());
        assert!(entry.worker.lock().unwrap().is_some());
        // The production registry deliberately retains failure authority; this test alone owns its unique key.
        entry.worker.lock().unwrap().take();
        INSTALLERS.lock().unwrap().remove(&path);
        assert!(weak.upgrade().is_none());
    }
}
