// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-process FIFO admission for bundled local inference.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use nix::time::{ClockId, clock_gettime};
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub enum AdmissionError {
    Timeout,
    Io(std::io::Error),
}

impl From<std::io::Error> for AdmissionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// A held local-inference slot (or every slot for exclusive admission).
///
/// Slot files remain on disk; dropping this permit only unlocks and closes them.
#[derive(Debug)]
pub struct LocalSlotPermit {
    pub slot_index: u32,
    pub queue_wait_ms: f64,
    _locks: Vec<File>,
}

struct WaitTicket {
    path: PathBuf,
    lock: Option<File>,
}

impl Drop for WaitTicket {
    fn drop(&mut self) {
        // Python unlinks before releasing its fd.  Keep the same ordering: an
        // error removing an already-raced path must not retain the flock.
        if let Err(error) = fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            // Drop cannot report the cleanup failure. The locked descriptor is
            // still released below, which is the important liveness property.
        }
        self.lock.take();
    }
}

pub fn admission_dir(journal_path: &Path) -> PathBuf {
    journal_path.join("health/local-inference-admission")
}

/// Acquire a FIFO local slot, optionally requiring every serving slot.
pub fn acquire_local_slot(
    root: &Path,
    capacity: u32,
    timeout: Option<Duration>,
    exclusive_admission: bool,
) -> Result<LocalSlotPermit, AdmissionError> {
    if capacity == 0 {
        return Err(AdmissionError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local admission capacity must be positive",
        )));
    }
    fs::create_dir_all(root)?;
    let started = Instant::now();
    let ticket = create_ticket(root)?;
    loop {
        if ticket_has_turn(root, &ticket.path)? {
            let permit = if exclusive_admission {
                try_acquire_exclusive(root, capacity, started)?
            } else {
                try_acquire(root, capacity, started)?
            };
            if let Some(permit) = permit {
                return Ok(permit);
            }
        }
        if timeout.is_some_and(|deadline| started.elapsed() >= deadline) {
            return Err(AdmissionError::Timeout);
        }
        let sleep_for = timeout
            .and_then(|deadline| deadline.checked_sub(started.elapsed()))
            .map(|remaining| remaining.min(POLL_INTERVAL))
            .unwrap_or(POLL_INTERVAL);
        if !sleep_for.is_zero() {
            thread::sleep(sleep_for);
        }
    }
}

fn create_ticket(root: &Path) -> Result<WaitTicket, AdmissionError> {
    let identity = ticket_identity()?;
    let creating = root.join(format!(".creating-{identity}.ticket"));
    let ticket = root.join(format!("wait-{identity}.ticket"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&creating)?;
    let lock = lock_exclusive(file).map_err(lock_error)?;
    if let Err(error) = fs::rename(&creating, &ticket) {
        drop(lock);
        let _ = fs::remove_file(&creating);
        return Err(error.into());
    }
    Ok(WaitTicket {
        path: ticket,
        lock: Some(lock),
    })
}

fn ticket_identity() -> Result<String, AdmissionError> {
    let nanos = monotonic_nanos()?;
    Ok(format!(
        "{nanos:020}-{}-{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ))
}

#[cfg(unix)]
fn monotonic_nanos() -> Result<u128, AdmissionError> {
    let now = clock_gettime(ClockId::CLOCK_MONOTONIC)
        .map_err(|error| std::io::Error::from_raw_os_error(error as i32))?;
    Ok(
        (u128::try_from(now.tv_sec()).unwrap_or_default() * 1_000_000_000)
            + u128::try_from(now.tv_nsec()).unwrap_or_default(),
    )
}

#[cfg(windows)]
fn monotonic_nanos() -> Result<u128, AdmissionError> {
    // SAFETY: GetTickCount64 reads the system's monotonic boot-duration counter
    // and has no pointer or ownership preconditions.
    #[allow(unsafe_code)]
    let milliseconds = unsafe { GetTickCount64() };
    Ok(u128::from(milliseconds) * 1_000_000)
}

#[cfg(not(any(unix, windows)))]
fn monotonic_nanos() -> Result<u128, AdmissionError> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos())
}

fn ticket_has_turn(root: &Path, own_ticket: &Path) -> Result<bool, AdmissionError> {
    loop {
        let Some(oldest) = wait_tickets(root)?.into_iter().next() else {
            return Ok(false);
        };
        if oldest == own_ticket {
            return Ok(true);
        }
        let file = match OpenOptions::new().read(true).write(true).open(&oldest) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        match lock_exclusive(file) {
            Ok(lock) => {
                let removed = fs::remove_file(&oldest);
                drop(lock);
                match removed {
                    Ok(()) => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn wait_tickets(root: &Path) -> Result<Vec<PathBuf>, AdmissionError> {
    let mut tickets = fs::read_dir(root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("wait-") && name.ends_with(".ticket")
            })
        })
        .collect::<Vec<_>>();
    tickets.sort();
    Ok(tickets)
}

fn try_acquire(
    root: &Path,
    capacity: u32,
    started: Instant,
) -> Result<Option<LocalSlotPermit>, AdmissionError> {
    for slot_index in 0..capacity {
        let file = slot_file(root, slot_index)?;
        match lock_exclusive(file) {
            Ok(lock) => {
                return Ok(Some(LocalSlotPermit {
                    slot_index,
                    queue_wait_ms: elapsed_ms(started),
                    _locks: vec![lock],
                }));
            }
            Err(TryLockError::WouldBlock) => continue,
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    Ok(None)
}

fn try_acquire_exclusive(
    root: &Path,
    capacity: u32,
    started: Instant,
) -> Result<Option<LocalSlotPermit>, AdmissionError> {
    let mut locks = Vec::with_capacity(capacity as usize);
    for slot_index in 0..capacity {
        let file = slot_file(root, slot_index)?;
        match lock_exclusive(file) {
            Ok(lock) => locks.push(lock),
            Err(TryLockError::WouldBlock) => {
                drop(locks);
                return Ok(None);
            }
            Err(TryLockError::Error(error)) => {
                drop(locks);
                return Err(error.into());
            }
        }
    }
    Ok(Some(LocalSlotPermit {
        slot_index: 0,
        queue_wait_ms: elapsed_ms(started),
        _locks: locks,
    }))
}

fn slot_file(root: &Path, slot_index: u32) -> Result<File, AdmissionError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(format!("slot-{slot_index}.lock")))
        .map_err(Into::into)
}

fn lock_exclusive(file: File) -> Result<File, TryLockError> {
    file.try_lock()?;
    Ok(file)
}

fn lock_error(error: TryLockError) -> AdmissionError {
    match error {
        TryLockError::WouldBlock => {
            AdmissionError::Io(std::io::Error::from(std::io::ErrorKind::WouldBlock))
        }
        TryLockError::Error(error) => error.into(),
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount64() -> u64;
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    fn root() -> PathBuf {
        let suffix = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "solstone-local-admission-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create test admission root");
        root
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capacity_and_release_are_process_safe() {
        let root = root();
        let first = acquire_local_slot(&root, 2, Some(Duration::from_millis(50)), false)
            .expect("first permit");
        let second = acquire_local_slot(&root, 2, Some(Duration::from_millis(50)), false)
            .expect("second permit");
        assert_ne!(first.slot_index, second.slot_index);
        assert!(matches!(
            acquire_local_slot(&root, 2, Some(Duration::from_millis(30)), false),
            Err(AdmissionError::Timeout)
        ));
        drop(first);
        let replacement = acquire_local_slot(&root, 2, Some(Duration::from_millis(50)), false)
            .expect("released slot is reusable");
        assert!(replacement.slot_index < 2);
        drop(replacement);
        drop(second);
        cleanup(&root);
    }

    #[test]
    fn stale_ticket_is_pruned_before_acquiring() {
        let root = root();
        fs::write(root.join("wait-00000000000000000000-1-dead.ticket"), b"")
            .expect("create stale ticket");
        let permit = acquire_local_slot(&root, 1, Some(Duration::from_millis(100)), false)
            .expect("stale ticket should be reclaimed");
        assert_eq!(permit.slot_index, 0);
        drop(permit);
        assert!(
            !root
                .join("wait-00000000000000000000-1-dead.ticket")
                .exists()
        );
        cleanup(&root);
    }

    #[test]
    fn exclusive_admission_holds_all_slots_but_normal_does_not() {
        let root = root();
        let exclusive = acquire_local_slot(&root, 2, Some(Duration::from_millis(50)), true)
            .expect("exclusive permit");
        assert!(matches!(
            acquire_local_slot(&root, 2, Some(Duration::from_millis(30)), false),
            Err(AdmissionError::Timeout)
        ));
        drop(exclusive);

        let normal = acquire_local_slot(&root, 2, Some(Duration::from_millis(50)), false)
            .expect("normal permit");
        let concurrent = acquire_local_slot(&root, 2, Some(Duration::from_millis(50)), false)
            .expect("normal admission must not take every slot");
        assert_ne!(normal.slot_index, concurrent.slot_index);
        drop(concurrent);
        drop(normal);
        cleanup(&root);
    }

    #[test]
    fn timeout_is_reported_after_waiting_for_the_head_slot() {
        let root = root();
        let held = acquire_local_slot(&root, 1, Some(Duration::from_millis(50)), false)
            .expect("held permit");
        assert!(matches!(
            acquire_local_slot(&root, 1, Some(Duration::from_millis(30)), false),
            Err(AdmissionError::Timeout)
        ));
        drop(held);
        cleanup(&root);
    }
}
