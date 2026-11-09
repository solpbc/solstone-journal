// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-process priority admission: bundled local inference's slots, and the
//! one slot older confidential calls share.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use solstone_core_journal_config::work_priority::WorkPriority;
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[cfg(target_os = "android")]
type AdmissionLock = nix::fcntl::Flock<File>;
#[cfg(not(target_os = "android"))]
type AdmissionLock = File;

#[derive(Debug)]
pub enum AdmissionError {
    Timeout,
    Io(std::io::Error),
}

#[derive(Debug)]
pub enum CheckedAdmissionError {
    Cancelled,
    Admission(AdmissionError),
}

impl From<AdmissionError> for CheckedAdmissionError {
    fn from(error: AdmissionError) -> Self {
        Self::Admission(error)
    }
}

impl From<std::io::Error> for CheckedAdmissionError {
    fn from(error: std::io::Error) -> Self {
        Self::Admission(AdmissionError::Io(error))
    }
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
    _locks: Vec<AdmissionLock>,
}

struct WaitTicket {
    path: PathBuf,
    lock: Option<AdmissionLock>,
}

impl Drop for WaitTicket {
    fn drop(&mut self) {
        // Unlink before releasing ownership; failed cleanup must not retain the lock.
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

/// The engine sends no retry interval, so a failed older confidential call
/// pauses older work for at least this long. It is the journal's own backoff.
const FAILURE_PAUSE: Duration = Duration::from_secs(60);
/// No pause is longer. One that ends further away than this was left by a
/// clock that has since moved back, and must not stall older work for days.
const LONGEST_PAUSE: Duration = Duration::from_secs(3600);
const PAUSE_FILE: &str = "next-call.json";

fn confidential_background_dir(journal_path: &Path) -> PathBuf {
    journal_path.join("health/confidential-background")
}

/// Run one confidential model call. Foreground calls run at once. Older calls
/// share the journal's one background slot, waiting for it within `timeout`,
/// and a failed one pauses older work. Nothing here sleeps but the slot wait:
/// the task queue starts older work only once [`confidential_background_open`],
/// so the pause spaces older tasks. The rest of a task already running is not
/// held by it: holding it would hold that task's lane.
pub fn run_confidential_call<T>(
    journal_path: &Path,
    priority: WorkPriority,
    timeout: Duration,
    call: impl FnOnce(Duration) -> T,
    failed: impl FnOnce(&T) -> bool,
) -> Result<T, String> {
    run_confidential_call_checked(journal_path, priority, timeout, &|| Ok(()), call, failed)
}

pub fn run_confidential_call_checked<T>(
    journal_path: &Path,
    priority: WorkPriority,
    timeout: Duration,
    checkpoint: &dyn Fn() -> Result<(), String>,
    call: impl FnOnce(Duration) -> T,
    failed: impl FnOnce(&T) -> bool,
) -> Result<T, String> {
    checkpoint()?;
    if !priority.band.is_background() {
        return Ok(call(timeout));
    }
    let waiting = Instant::now();
    let root = confidential_background_dir(journal_path);
    let _permit = acquire_local_slot(&root, 1, Some(timeout), false, priority)
        .map_err(|error| format!("confidential background admission: {error:?}"))?;
    checkpoint()?;
    let remaining = timeout
        .checked_sub(waiting.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or("confidential background wait exhausted the request timeout")?;
    let started = Instant::now();
    let result = call(remaining);
    if failed(&result) {
        checkpoint()?;
        let pause = started.elapsed().clamp(FAILURE_PAUSE, LONGEST_PAUSE);
        let until = SystemTime::now() + pause;
        // Best effort: the call already failed, and its own result says why.
        let _ = pause_until(&root, until);
    }
    Ok(result)
}

/// Whether older confidential work may start now: no call holds or awaits the
/// background slot and no failed call's pause is still running. Never waits.
/// An unreadable slot fails closed; foreground work never consults it.
pub fn confidential_background_open(journal_path: &Path) -> bool {
    let root = confidential_background_dir(journal_path);
    let now = SystemTime::now();
    let paused = paused_until(&root);
    let moved_clock = paused > now + LONGEST_PAUSE + FAILURE_PAUSE;
    (paused <= now || moved_clock) && is_idle(&root, 1).unwrap_or(false)
}

fn pause_until(root: &Path, until: SystemTime) -> std::io::Result<()> {
    // Round up: a pause must not end before the time it was asked for.
    let millis = until
        .duration_since(UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos()
        .div_ceil(1_000_000);
    let staged = root.join(format!(".{PAUSE_FILE}-{}", Uuid::new_v4().simple()));
    fs::write(&staged, millis.to_string())?;
    fs::rename(&staged, root.join(PAUSE_FILE)).inspect_err(|_| {
        let _ = fs::remove_file(&staged);
    })
}

/// An unreadable pause is no pause: it is advice, and damage must not stop work.
fn paused_until(root: &Path) -> SystemTime {
    fs::read(root.join(PAUSE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<u64>(&bytes).ok())
        .and_then(|millis| UNIX_EPOCH.checked_add(Duration::from_millis(millis)))
        .unwrap_or(UNIX_EPOCH)
}

/// Whether a caller could take a slot now without waiting behind anyone. A
/// held slot or a live waiter answers `false` at once.
fn is_idle(root: &Path, capacity: u32) -> Result<bool, AdmissionError> {
    let tickets = match wait_tickets(root) {
        Err(AdmissionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(true);
        }
        tickets => tickets?,
    };
    // A ticket whose lock is free belongs to a waiter that has gone.
    let slots = (0..capacity).map(|index| root.join(format!("slot-{index}.lock")));
    for path in tickets.into_iter().chain(slots) {
        let file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) if is_delete_pending(&error) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        match lock_exclusive(file) {
            Ok(_free) => {}
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    Ok(true)
}

/// Acquire the highest-ranked waiting call, optionally requiring every slot.
pub fn acquire_local_slot(
    root: &Path,
    capacity: u32,
    timeout: Option<Duration>,
    exclusive_admission: bool,
    priority: WorkPriority,
) -> Result<LocalSlotPermit, AdmissionError> {
    match acquire_local_slot_checked(
        root,
        capacity,
        timeout,
        exclusive_admission,
        priority,
        &|| false,
        &mut || {},
    ) {
        Ok(permit) => Ok(permit),
        Err(CheckedAdmissionError::Admission(error)) => Err(error),
        Err(CheckedAdmissionError::Cancelled) => unreachable!("uncancellable admission"),
    }
}

/// The same priority queue with an owner/generation checkpoint. `on_wait` runs
/// once, only after immediate admission failed, with arbitration released. A
/// caller may release an idle cached native model before waiting for compute.
#[allow(clippy::too_many_arguments)]
pub fn acquire_local_slot_checked(
    root: &Path,
    capacity: u32,
    timeout: Option<Duration>,
    exclusive_admission: bool,
    priority: WorkPriority,
    cancelled: &dyn Fn() -> bool,
    on_wait: &mut dyn FnMut(),
) -> Result<LocalSlotPermit, CheckedAdmissionError> {
    let checkpoint = || {
        if cancelled() {
            Err(CheckedAdmissionError::Cancelled)
        } else {
            Ok(())
        }
    };
    checkpoint()?;
    if capacity == 0 {
        return Err(AdmissionError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local admission capacity must be positive",
        ))
        .into());
    }
    let started = Instant::now();
    fs::create_dir_all(root)?;
    let mut ticket = None;
    let mut notified_wait = false;
    loop {
        checkpoint()?;
        check_deadline(started, timeout)?;
        if let Some(arbitration) = try_arbitrate(root)? {
            checkpoint()?;
            check_deadline(started, timeout)?;
            if ticket.is_none() {
                ticket = Some(create_ticket(root, priority)?);
            }
            if ticket_has_turn(root, &ticket.as_ref().expect("published ticket").path)? {
                check_deadline(started, timeout)?;
                let permit = if exclusive_admission {
                    try_acquire_exclusive(root, capacity, started)?
                } else {
                    try_acquire(root, capacity, started)?
                };
                if let Some(permit) = permit {
                    checkpoint()?;
                    check_deadline(started, timeout)?;
                    // Publication and selection share arbitration. Retire this
                    // waiter before letting another caller select the next one.
                    drop(ticket.take());
                    return Ok(permit);
                }
            }
            drop(arbitration);
        }
        if !notified_wait {
            on_wait();
            notified_wait = true;
        }
        checkpoint()?;
        check_deadline(started, timeout)?;
        let sleep_for = timeout
            .and_then(|deadline| deadline.checked_sub(started.elapsed()))
            .map(|remaining| remaining.min(POLL_INTERVAL))
            .unwrap_or(POLL_INTERVAL);
        if !sleep_for.is_zero() {
            thread::sleep(sleep_for);
        }
    }
}

fn check_deadline(started: Instant, timeout: Option<Duration>) -> Result<(), AdmissionError> {
    if timeout.is_some_and(|deadline| started.elapsed() >= deadline) {
        Err(AdmissionError::Timeout)
    } else {
        Ok(())
    }
}

fn try_arbitrate(root: &Path) -> Result<Option<AdmissionLock>, AdmissionError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("arbitration.lock"))?;
    match lock_exclusive(file) {
        Ok(lock) => Ok(Some(lock)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

// The caller holds arbitration. Sequence is after every published waiter;
// clocks, platform timer resolution and process ids cannot reverse equal keys.
fn create_ticket(root: &Path, priority: WorkPriority) -> Result<WaitTicket, AdmissionError> {
    let sequence = wait_tickets(root)?
        .iter()
        .filter_map(|path| {
            path.file_name()?
                .to_str()?
                .split('-')
                .nth(3)?
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("local admission sequence exhausted"))?;
    let identity = format!(
        "{}-{:020}-{sequence:020}-{}",
        priority.band as u8,
        u64::MAX - priority.source_time,
        Uuid::new_v4().simple()
    );
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
            // Windows refuses to open a file whose delete is still pending (its
            // owner is leaving the queue). Wait for the next poll, as for a held ticket.
            Err(error) if is_delete_pending(&error) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        match lock_exclusive(file) {
            Ok(lock) => {
                let removed = fs::remove_file(&oldest);
                drop(lock);
                match removed {
                    Ok(()) => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) if is_delete_pending(&error) => return Ok(false),
                    Err(error) => return Err(error.into()),
                }
            }
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

#[cfg(windows)]
fn is_delete_pending(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
}

#[cfg(not(windows))]
fn is_delete_pending(_error: &std::io::Error) -> bool {
    false
}

fn wait_tickets(root: &Path) -> Result<Vec<PathBuf>, AdmissionError> {
    let mut tickets = fs::read_dir(root)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
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

#[cfg(not(target_os = "android"))]
fn lock_exclusive(file: File) -> Result<AdmissionLock, TryLockError> {
    file.try_lock()?;
    Ok(file)
}

#[cfg(target_os = "android")]
fn lock_exclusive(file: File) -> Result<AdmissionLock, TryLockError> {
    // The pinned Rust standard library does not implement File::try_lock on
    // Android. Bionic flock provides the same owned, nonblocking lock contract.
    nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock).map_err(
        |(_file, error)| {
            if error == nix::errno::Errno::EWOULDBLOCK {
                TryLockError::WouldBlock
            } else {
                TryLockError::Error(std::io::Error::from(error))
            }
        },
    )
}

fn lock_error(error: TryLockError) -> AdmissionError {
    match error {
        TryLockError::WouldBlock => {
            AdmissionError::Io(std::io::Error::from(std::io::ErrorKind::WouldBlock))
        }
        TryLockError::Error(error) => error.into(),
    }
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::test_support::lock_released;

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn checked_wait_cancels_without_native_entry_and_retires_its_ticket() {
        use std::sync::{Arc, atomic::AtomicBool, mpsc};
        let root = tempfile::tempdir().unwrap();
        let held =
            acquire_local_slot(root.path(), 1, None, false, WorkPriority::default()).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (queued, observed) = mpsc::sync_channel(1);
        let worker_root = root.path().to_owned();
        let worker_cancelled = Arc::clone(&cancelled);
        let entered = Arc::new(AtomicBool::new(false));
        let worker_entered = Arc::clone(&entered);
        let worker = thread::spawn(move || {
            let permit = acquire_local_slot_checked(
                &worker_root,
                1,
                Some(Duration::from_secs(2)),
                false,
                WorkPriority::default(),
                &|| worker_cancelled.load(Ordering::Acquire),
                &mut || {
                    queued.send(()).unwrap();
                },
            );
            if permit.is_ok() {
                worker_entered.store(true, Ordering::Release);
            }
            permit
        });
        observed.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(wait_tickets(root.path()).unwrap().len(), 1);
        cancelled.store(true, Ordering::Release);
        assert!(matches!(
            worker.join().unwrap(),
            Err(CheckedAdmissionError::Cancelled)
        ));
        assert!(!entered.load(Ordering::Acquire));
        assert!(wait_tickets(root.path()).unwrap().is_empty());
        drop(held);
        assert!(
            acquire_local_slot(
                root.path(),
                1,
                Some(ACQUIRE_DEADLINE),
                false,
                WorkPriority::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn checked_immediate_path_keeps_cache_and_precancel_publishes_no_ticket() {
        use std::cell::Cell;
        let root = tempfile::tempdir().unwrap();
        let evictions = Cell::new(0);
        let permit = acquire_local_slot_checked(
            root.path(),
            1,
            None,
            false,
            WorkPriority::default(),
            &|| false,
            &mut || evictions.set(evictions.get() + 1),
        )
        .unwrap();
        assert_eq!(evictions.get(), 0);
        drop(permit);
        let result = acquire_local_slot_checked(
            root.path(),
            1,
            None,
            false,
            WorkPriority::default(),
            &|| true,
            &mut || evictions.set(evictions.get() + 1),
        );
        assert!(matches!(result, Err(CheckedAdmissionError::Cancelled)));
        assert_eq!(evictions.get(), 0);
        assert!(wait_tickets(root.path()).unwrap().is_empty());
        assert!(
            acquire_local_slot(
                root.path(),
                1,
                Some(ACQUIRE_DEADLINE),
                false,
                WorkPriority::default()
            )
            .is_ok()
        );
    }

    /// Deadline for an acquisition the test expects to succeed. A loaded
    /// Windows runner can take well over 100 ms to create and lock the first
    /// files of a fresh admission root at test-binary start
    /// (winci-overnight-261012-j42-b1); only the expected-timeout probes stay short.
    const ACQUIRE_DEADLINE: Duration = Duration::from_secs(5);

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
        let first = acquire_local_slot(
            &root,
            2,
            Some(ACQUIRE_DEADLINE),
            false,
            WorkPriority::default(),
        )
        .expect("first permit");
        let second = acquire_local_slot(
            &root,
            2,
            Some(ACQUIRE_DEADLINE),
            false,
            WorkPriority::default(),
        )
        .expect("second permit");
        assert_ne!(first.slot_index, second.slot_index);
        assert!(matches!(
            acquire_local_slot(
                &root,
                2,
                Some(Duration::from_millis(30)),
                false,
                WorkPriority::default()
            ),
            Err(AdmissionError::Timeout)
        ));
        drop(first);
        let replacement = acquire_local_slot(
            &root,
            2,
            Some(ACQUIRE_DEADLINE),
            false,
            WorkPriority::default(),
        )
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
        let permit = acquire_local_slot(
            &root,
            1,
            Some(ACQUIRE_DEADLINE),
            false,
            WorkPriority::default(),
        )
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
        let exclusive = acquire_local_slot(
            &root,
            2,
            Some(ACQUIRE_DEADLINE),
            true,
            WorkPriority::default(),
        )
        .expect("exclusive permit");
        assert!(matches!(
            acquire_local_slot(
                &root,
                2,
                Some(Duration::from_millis(30)),
                false,
                WorkPriority::default()
            ),
            Err(AdmissionError::Timeout)
        ));
        drop(exclusive);

        let normal = acquire_local_slot(
            &root,
            2,
            Some(ACQUIRE_DEADLINE),
            false,
            WorkPriority::default(),
        )
        .expect("normal permit");
        let concurrent = acquire_local_slot(
            &root,
            2,
            Some(ACQUIRE_DEADLINE),
            false,
            WorkPriority::default(),
        )
        .expect("normal admission must not take every slot");
        assert_ne!(normal.slot_index, concurrent.slot_index);
        drop(concurrent);
        drop(normal);
        cleanup(&root);
    }

    #[cfg(windows)]
    #[test]
    fn a_ticket_whose_delete_is_pending_is_waited_on_not_an_error() {
        use std::os::windows::fs::OpenOptionsExt;
        const DELETE: u32 = 0x0001_0000;
        const GENERIC_READ: u32 = 0x8000_0000;
        const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
        let root = root();
        let leaving = root.join("wait-00000000000000000001-1-leaving.ticket");
        let own = root.join("wait-99999999999999999999-1-own.ticket");
        fs::write(&leaving, b"").expect("create the leaving ticket");
        fs::write(&own, b"").expect("create our ticket");
        let still_open = File::open(&leaving).expect("a second handle keeps it on disk");
        let deleting = OpenOptions::new()
            .access_mode(GENERIC_READ | DELETE)
            .share_mode(0x7)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .open(&leaving)
            .expect("open with delete on close");
        drop(deleting);
        let refused = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&leaving)
            .expect_err("a delete-pending file cannot be opened");
        assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(matches!(ticket_has_turn(&root, &own), Ok(false)));
        drop(still_open);
        assert!(matches!(ticket_has_turn(&root, &own), Ok(true)));
        cleanup(&root);
    }

    #[test]
    fn timeout_is_reported_after_waiting_for_the_head_slot() {
        let root = root();
        let held = acquire_local_slot(
            &root,
            1,
            Some(ACQUIRE_DEADLINE),
            false,
            WorkPriority::default(),
        )
        .expect("held permit");
        assert!(matches!(
            acquire_local_slot(
                &root,
                1,
                Some(Duration::from_millis(30)),
                false,
                WorkPriority::default()
            ),
            Err(AdmissionError::Timeout)
        ));
        drop(held);
        cleanup(&root);
    }

    fn await_tickets(root: &Path, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while wait_tickets(root).unwrap().len() != count {
            assert!(Instant::now() < deadline, "waiters did not publish");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn published_waiters_admit_by_band_newest_and_equal_key_fifo() {
        let root = tempfile::tempdir().unwrap();
        let held = acquire_local_slot(
            root.path(),
            1,
            Some(Duration::from_secs(5)),
            false,
            WorkPriority::default(),
        )
        .unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let inputs = ["6:9", "5:9", "4:9", "3:9", "2:9", "1:1", "1:9", "1:9"];
        let mut workers = Vec::new();
        for (index, input) in inputs.into_iter().enumerate() {
            let path = root.path().to_owned();
            let sent = sent.clone();
            workers.push(thread::spawn(move || {
                let permit = acquire_local_slot(
                    &path,
                    1,
                    Some(Duration::from_secs(10)),
                    false,
                    input.parse().unwrap(),
                )
                .unwrap();
                sent.send(index).unwrap();
                drop(permit);
            }));
            await_tickets(root.path(), index + 1);
        }
        drop(held);
        for expected in [6, 7, 5, 4, 3, 2, 1, 0] {
            assert_eq!(
                received.recv_timeout(Duration::from_secs(5)).unwrap(),
                expected
            );
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(wait_tickets(root.path()).unwrap().is_empty());
    }

    #[test]
    fn higher_normal_waiter_passes_lower_exclusive_without_interrupting_running_call() {
        let root = tempfile::tempdir().unwrap();
        let running =
            acquire_local_slot(root.path(), 2, None, false, WorkPriority::default()).unwrap();
        let path = root.path().to_owned();
        let lower = thread::spawn(move || {
            acquire_local_slot(
                &path,
                2,
                Some(Duration::from_secs(5)),
                true,
                "6:9".parse().unwrap(),
            )
            .unwrap()
        });
        await_tickets(root.path(), 1);
        let high = acquire_local_slot(
            root.path(),
            2,
            Some(Duration::from_secs(2)),
            false,
            "1:9".parse().unwrap(),
        )
        .unwrap();
        assert_ne!(running.slot_index, high.slot_index);
        assert!(!lower.is_finished());
        drop(high);
        drop(running);
        drop(lower.join().unwrap());
    }

    #[test]
    fn arbitration_serializes_publication_and_admission_and_obeys_deadline() {
        let root = tempfile::tempdir().unwrap();
        let held =
            acquire_local_slot(root.path(), 1, None, false, WorkPriority::default()).unwrap();
        let path = root.path().to_owned();
        let low = thread::spawn(move || {
            acquire_local_slot(
                &path,
                1,
                Some(Duration::from_millis(100)),
                false,
                "6:9".parse().unwrap(),
            )
        });
        await_tickets(root.path(), 1);
        let arbitration = loop {
            if let Some(lock) = try_arbitrate(root.path()).unwrap() {
                break lock;
            }
            thread::yield_now();
        };
        drop(held);
        let path = root.path().to_owned();
        let high = thread::spawn(move || {
            acquire_local_slot(
                &path,
                1,
                Some(Duration::from_millis(100)),
                false,
                "1:9".parse().unwrap(),
            )
        });
        assert!(matches!(high.join().unwrap(), Err(AdmissionError::Timeout)));
        assert!(matches!(low.join().unwrap(), Err(AdmissionError::Timeout)));
        assert!(wait_tickets(root.path()).unwrap().is_empty());
        drop(arbitration);
        assert!(matches!(
            acquire_local_slot(
                root.path(),
                1,
                Some(Duration::ZERO),
                false,
                WorkPriority::default()
            ),
            Err(AdmissionError::Timeout)
        ));
        drop(
            acquire_local_slot(
                root.path(),
                1,
                Some(Duration::from_secs(1)),
                false,
                WorkPriority::default(),
            )
            .unwrap(),
        );
    }

    #[test]
    fn older_confidential_calls_share_one_slot_and_a_failure_pauses_older_work() {
        let journal = tempfile::tempdir().unwrap();
        let root = confidential_background_dir(journal.path());
        let older: WorkPriority = "5:1".parse().unwrap();
        assert!(confidential_background_open(journal.path()));
        let held = acquire_local_slot(&root, 1, Some(ACQUIRE_DEADLINE), false, older).unwrap();
        assert!(!confidential_background_open(journal.path()));
        for band in 1..=3 {
            let priority = format!("{band}:1").parse().unwrap();
            let ran =
                run_confidential_call(journal.path(), priority, Duration::ZERO, |_| 7, |_| false);
            assert_eq!(ran, Ok(7), "foreground band {band} bypasses the slot");
        }
        let waited = run_confidential_call(
            journal.path(),
            older,
            Duration::from_millis(30),
            |_| panic!("the slot is held"),
            |_: &()| false,
        );
        assert!(waited.is_err());
        drop(held);

        let ran =
            run_confidential_call(journal.path(), older, ACQUIRE_DEADLINE, |_| "ok", |_| false);
        assert_eq!(ran, Ok("ok"));
        assert!(
            lock_released(|| confidential_background_open(journal.path())),
            "success leaves no pause"
        );

        let before = SystemTime::now();
        let ran =
            run_confidential_call(journal.path(), older, ACQUIRE_DEADLINE, |_| "no", |_| true);
        assert_eq!(ran, Ok("no"));
        assert!(paused_until(&root) >= before + FAILURE_PAUSE);
        assert!(!confidential_background_open(journal.path()));
        // The pause is the queue's to honour: an admitted call is not held by it.
        let ran =
            run_confidential_call(journal.path(), older, ACQUIRE_DEADLINE, |_| "ok", |_| false);
        assert_eq!(ran, Ok("ok"));
    }

    #[test]
    fn cancellation_after_background_slot_wait_refuses_content_and_preserves_the_positive_twin() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        };
        for cancel in [false, true] {
            let journal = tempfile::tempdir().unwrap();
            let root = confidential_background_dir(journal.path());
            let priority: WorkPriority = "4:20200101120000".parse().unwrap();
            let held =
                acquire_local_slot(&root, 1, Some(ACQUIRE_DEADLINE), false, priority).unwrap();
            let cancelled = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&cancelled);
            let calls = Arc::new(AtomicUsize::new(0));
            let count = Arc::clone(&calls);
            let path = journal.path().to_path_buf();
            let (checking, reached) = mpsc::channel();
            let (ended, done) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let result = run_confidential_call_checked(
                    &path,
                    priority,
                    ACQUIRE_DEADLINE,
                    &|| {
                        checking.send(()).unwrap();
                        if flag.load(Ordering::Acquire) {
                            Err("cancelled".into())
                        } else {
                            Ok(())
                        }
                    },
                    |_| {
                        count.fetch_add(1, Ordering::SeqCst);
                        "ok"
                    },
                    |_| false,
                );
                ended.send(result).unwrap();
            });
            reached.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(
                done.recv_timeout(Duration::from_millis(50)).is_err(),
                "the real background slot is held"
            );
            cancelled.store(cancel, Ordering::Release);
            drop(held);
            let result = done.recv_timeout(Duration::from_secs(5)).unwrap();
            worker.join().unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(!cancel));
            if cancel {
                assert_eq!(result.unwrap_err(), "cancelled");
            } else {
                assert_eq!(result.unwrap(), "ok");
            }
            assert!(
                lock_released(|| confidential_background_open(journal.path())),
                "cancellation must not publish a failure pause"
            );
        }
    }

    #[test]
    fn cancellation_after_background_work_does_not_publish_a_late_failure_pause() {
        use std::cell::Cell;
        for cancel in [false, true] {
            let journal = tempfile::tempdir().unwrap();
            let cancelled = Cell::new(false);
            let result = run_confidential_call_checked(
                journal.path(),
                "4:1".parse().unwrap(),
                ACQUIRE_DEADLINE,
                &|| {
                    if cancelled.get() {
                        Err("cancelled".into())
                    } else {
                        Ok(())
                    }
                },
                |_| {
                    cancelled.set(cancel);
                    "failed"
                },
                |_| true,
            );
            if cancel {
                assert_eq!(result.unwrap_err(), "cancelled");
                assert!(lock_released(|| confidential_background_open(
                    journal.path()
                )));
            } else {
                assert_eq!(result.unwrap(), "failed");
                assert!(!confidential_background_open(journal.path()));
            }
        }
    }

    #[test]
    fn the_open_check_never_waits_and_sees_holders_waiters_and_pauses() {
        let journal = tempfile::tempdir().unwrap();
        let root = confidential_background_dir(journal.path());
        fs::create_dir_all(&root).unwrap();
        let live = root.join("wait-5-00000000000000000001-00000000000000000001-live.ticket");
        let waiter = File::create(&live).unwrap();
        waiter.try_lock().unwrap();
        assert!(
            !confidential_background_open(journal.path()),
            "a live waiter"
        );
        drop(waiter);
        assert!(
            lock_released(|| confidential_background_open(journal.path())),
            "a gone waiter"
        );
        let pause = |text: &str| fs::write(root.join(PAUSE_FILE), text).unwrap();
        for damage in ["not a time".to_owned(), u64::MAX.to_string()] {
            pause(&damage);
            assert!(
                confidential_background_open(journal.path()),
                "damage is no pause"
            );
        }
        let millis = |at: SystemTime| {
            at.duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .to_string()
        };
        pause(&millis(SystemTime::now() + Duration::from_secs(60)));
        assert!(!confidential_background_open(journal.path()));
        pause(&millis(SystemTime::now() - Duration::from_secs(1)));
        assert!(confidential_background_open(journal.path()));
        pause(&millis(SystemTime::now() + 2 * LONGEST_PAUSE));
        assert!(
            confidential_background_open(journal.path()),
            "a clock moved back"
        );
    }
}
