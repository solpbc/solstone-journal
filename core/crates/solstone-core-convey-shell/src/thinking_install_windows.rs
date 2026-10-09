// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Installer control keeps the original Job, including after the root exits.

use super::{ADMISSION_TIMEOUT, STOP_TIMEOUT, lease, status};
use serde_json::Value;
use solstone_core_system::process::{LaunchAuthority, ProcessInstance};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

const COMPLETED_LIMIT: usize = 32;
static OWNERS: Mutex<Vec<Arc<Owner>>> = Mutex::new(Vec::new());

struct Owner {
    instance: ProcessInstance,
    journal: PathBuf,
    child: Mutex<Option<LaunchAuthority>>,
    completed: AtomicBool,
    cleanup_pending: AtomicBool,
}

fn lock_child(
    owner: &Owner,
    deadline: Instant,
) -> Result<MutexGuard<'_, Option<LaunchAuthority>>, String> {
    loop {
        match owner.child.try_lock() {
            Ok(child) => return Ok(child),
            Err(TryLockError::Poisoned(_)) => return Err("installer control unavailable".into()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => return Err("installer control timed out".into()),
        }
    }
}

fn register(mut child: LaunchAuthority, journal: &Path) -> Result<Arc<Owner>, String> {
    let Some(instance) = child.process_instance() else {
        child
            .terminate_exact(STOP_TIMEOUT)
            .map_err(|error| error.to_string())?;
        return Err("installer identity unavailable".into());
    };
    let owner = Arc::new(Owner {
        instance,
        journal: journal.to_owned(),
        child: Mutex::new(Some(child)),
        completed: AtomicBool::new(false),
        cleanup_pending: AtomicBool::new(false),
    });
    let mut owners = OWNERS
        .lock()
        .map_err(|_| "installer registry unavailable")?;
    // Only completed receipts are evicted. Failed cleanup keeps its Job handle.
    prune_completed(&mut owners);
    owners.push(Arc::clone(&owner));
    Ok(owner)
}

fn prune_completed(owners: &mut Vec<Arc<Owner>>) {
    let mut completed = owners
        .iter()
        .filter(|owner| owner.completed.load(Ordering::Acquire))
        .count();
    owners.retain(|owner| {
        if completed > COMPLETED_LIMIT && owner.completed.load(Ordering::Acquire) {
            completed -= 1;
            false
        } else {
            true
        }
    });
}

fn find(instance: ProcessInstance) -> Result<Option<Arc<Owner>>, String> {
    Ok(OWNERS
        .lock()
        .map_err(|_| "installer registry unavailable")?
        .iter()
        .find(|owner| owner.instance == instance)
        .cloned())
}

fn retire(owner: &Owner) -> Result<(), String> {
    let deadline = Instant::now() + ADMISSION_TIMEOUT;
    let mut child = lock_child(owner, deadline)?;
    let Some(authority) = child.as_mut() else {
        return Ok(());
    };
    owner.cleanup_pending.store(true, Ordering::Release);
    authority
        .terminate_exact(STOP_TIMEOUT.min(deadline.saturating_duration_since(Instant::now())))
        .map_err(|error| format!("installer Job cleanup failed: {error}"))?;
    let retired = child.take();
    owner.completed.store(true, Ordering::Release);
    owner.cleanup_pending.store(false, Ordering::Release);
    drop(child);
    if let Ok(mut owners) = OWNERS.lock() {
        prune_completed(&mut owners);
    }
    // The Job is confirmed quiescent. Log drains have their own bounded Drop;
    // never hold either registry or control lock across that destruction.
    drop(retired);
    Ok(())
}

pub(super) fn stop(instance: ProcessInstance) -> Result<(), String> {
    // The child can publish its status in the small launch-to-register interval.
    // Wait for that registration, never substitute PID observation as authority.
    let deadline = Instant::now() + ADMISSION_TIMEOUT;
    loop {
        if let Some(owner) = find(instance)? {
            return retire(&owner);
        }
        if Instant::now() >= deadline {
            return Err("installer Job authority unavailable".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn stop_registered(value: &Value) -> Result<(), String> {
    let instance = serde_json::from_value(value.clone())
        .map_err(|_| "installer process identity unavailable")?;
    if let Some(owner) = find(instance)? {
        retire(&owner)?;
    }
    Ok(())
}

pub(super) fn reconcile(journal: &Path) -> Result<(), String> {
    let owners: Vec<_> = OWNERS
        .lock()
        .map_err(|_| "installer registry unavailable")?
        .iter()
        .filter(|owner| owner.journal == journal && !owner.completed.load(Ordering::Acquire))
        .cloned()
        .collect();
    for owner in owners {
        if owner.cleanup_pending.load(Ordering::Acquire) || !matches!(poll(&owner), Ok(None)) {
            retire(&owner)?;
        }
    }
    Ok(())
}

fn poll(owner: &Owner) -> Result<Option<i32>, String> {
    let mut child = lock_child(owner, Instant::now() + ADMISSION_TIMEOUT)?;
    match child.as_mut() {
        Some(child) => child.poll().map_err(|error| error.to_string()),
        None => Err("installer was stopped".into()),
    }
}

pub(super) fn admit(
    child: LaunchAuthority,
    journal: &Path,
    model: &str,
    timeout: Duration,
    admitted: &std::sync::mpsc::SyncSender<Result<Value, String>>,
) {
    let owner = match register(child, journal) {
        Ok(owner) => owner,
        Err(error) => {
            let _ = admitted.send(Err(error));
            return;
        }
    };
    let outcome = admission(&owner, journal, model, timeout);
    match outcome {
        Ok((value, true)) => {
            if admitted.send(Ok(value)).is_err() {
                if let Err(error) = retire(&owner) {
                    log::error!("{error}");
                }
                return;
            }
            // This thread retains monitoring through the whole download. It
            // never holds a mutex while waiting for the installer to exit.
            loop {
                match poll(&owner) {
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Ok(Some(code)) => {
                        record_unfinished_exit(&owner, code);
                        if let Err(error) = retire(&owner) {
                            log::error!("{error}");
                        }
                        return;
                    }
                    Err(_) => {
                        if let Err(error) = retire(&owner) {
                            log::error!("{error}");
                        }
                        return;
                    }
                }
            }
        }
        result => {
            let result = match retire(&owner) {
                Ok(()) => result.map(|(value, _)| value),
                Err(error) => Err(error),
            };
            let _ = admitted.send(result);
        }
    }
}

/// The installer writes its own terminal status. When it exits without one
/// (killed, crashed), its attempt would read in-flight with nothing running,
/// so that attempt, and only that attempt, is recorded as interrupted. A
/// status another attempt owns, or one already terminal, is left alone.
fn record_unfinished_exit(owner: &Owner, code: i32) {
    let current = match status::read_status(&owner.journal, "local") {
        Ok(current) => current,
        Err(error) => {
            log::warn!("local model installer exited ({code}); its status is unreadable: {error}");
            return;
        }
    };
    let owned = current
        .owner
        .clone()
        .and_then(|value| serde_json::from_value::<ProcessInstance>(value).ok())
        == Some(owner.instance);
    let attempt = current.attempt_id.as_deref().filter(|_| owned);
    let Some(attempt) = attempt.filter(|_| status::is_in_flight(&current.install_state)) else {
        if code != 0 {
            log::warn!(
                "local model installer exited ({code}); install state {}",
                current.install_state
            );
        }
        return;
    };
    match status::record_interrupted(
        &owner.journal,
        attempt,
        current.target_fingerprint_sha256.as_deref(),
    ) {
        Ok(_) => log::warn!(
            "local model installer exited ({code}) while {}; recorded as interrupted",
            current.install_state
        ),
        Err(error) => log::warn!(
            "local model installer exited ({code}) while {}; not recorded: {error}",
            current.install_state
        ),
    }
}

fn admission(
    owner: &Owner,
    journal: &Path,
    model: &str,
    timeout: Duration,
) -> Result<(Value, bool), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let exit = poll(owner)?;
        let current = status::read_status(journal, "local").map_err(|error| error.to_string())?;
        let held = lease::is_held(journal, "local").map_err(|error| error.to_string())?;
        let owned = current
            .owner
            .clone()
            .and_then(|value| serde_json::from_value::<ProcessInstance>(value).ok())
            == Some(owner.instance);
        let in_flight =
            current.attempt_id.is_some() && status::is_in_flight(&current.install_state) && held;
        if owned && in_flight && exit.is_none() {
            return Ok((
                solstone_core_thinking::local::bootstrap_status(journal, model),
                true,
            ));
        }
        if let Some(code) = exit {
            if (code == 0 && !held && current.install_state == "installed") || in_flight {
                // A losing launch may report the independent lease winner, but
                // retires only its own original Job before returning that value.
                return Ok((
                    solstone_core_thinking::local::bootstrap_status(journal, model),
                    false,
                ));
            }
            return Err(format!("installer exited before admission ({code})"));
        }
        if Instant::now() >= deadline {
            return Err("installer admission timed out".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
#[path = "thinking_install_windows_tests.rs"]
mod tests;
