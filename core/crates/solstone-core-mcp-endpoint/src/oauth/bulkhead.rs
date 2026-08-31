// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-memory CIMD fetch bulkhead.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;
use tokio::time::{Instant, sleep_until};

use super::cimd::canonicalize_ip;

const CIMD_GLOBAL_PERMITS: usize = 16;
const CIMD_PER_SOURCE_PERMITS: usize = 2;
const CIMD_BULKHEAD_MAX_KEYS: usize = 16;

/// Why a waiting CIMD acquire cannot proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CimdBulkheadError {
    Timeout,
    #[allow(dead_code)]
    Cancelled,
}

struct CimdBulkheadState {
    global: usize,
    per_source: HashMap<IpAddr, usize>,
}

/// Bounded concurrent CIMD-fetch admission.
pub(crate) struct CimdBulkhead {
    inner: Mutex<CimdBulkheadState>,
    notify: Notify,
}

/// One held CIMD fetch permit. Drop releases it.
pub(crate) struct CimdPermit {
    bulkhead: Arc<CimdBulkhead>,
    source: IpAddr,
}

impl Drop for CimdPermit {
    fn drop(&mut self) {
        self.bulkhead.release(self.source);
    }
}

impl CimdBulkhead {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(CimdBulkheadState {
                global: 0,
                per_source: HashMap::new(),
            }),
            notify: Notify::new(),
        })
    }

    pub(crate) fn try_acquire(self: &Arc<Self>, source: IpAddr) -> Option<CimdPermit> {
        let source = canonicalize_ip(source);
        let mut state = lock(&self.inner);
        if state.global >= CIMD_GLOBAL_PERMITS {
            return None;
        }
        let held = state.per_source.get(&source).copied().unwrap_or(0);
        if held >= CIMD_PER_SOURCE_PERMITS {
            return None;
        }
        if held == 0 && state.per_source.len() >= CIMD_BULKHEAD_MAX_KEYS {
            return None;
        }
        state.global += 1;
        state.per_source.insert(source, held + 1);
        Some(CimdPermit {
            bulkhead: Arc::clone(self),
            source,
        })
    }

    pub(crate) async fn acquire(
        self: &Arc<Self>,
        source: IpAddr,
        deadline: Instant,
    ) -> Result<CimdPermit, CimdBulkheadError> {
        loop {
            let notified = self.notify.notified();
            if let Some(permit) = self.try_acquire(source) {
                return Ok(permit);
            }
            if Instant::now() >= deadline {
                return Err(CimdBulkheadError::Timeout);
            }
            tokio::select! {
                biased;
                () = notified => {}
                () = sleep_until(deadline) => return Err(CimdBulkheadError::Timeout),
            }
        }
    }

    fn release(&self, source: IpAddr) {
        let source = canonicalize_ip(source);
        let mut state = lock(&self.inner);
        state.global = state.global.saturating_sub(1);
        if let Some(held) = state.per_source.get_mut(&source) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                state.per_source.remove(&source);
            }
        }
        drop(state);
        self.notify.notify_waiters();
    }
}

fn lock(mutex: &Mutex<CimdBulkheadState>) -> std::sync::MutexGuard<'_, CimdBulkheadState> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    use tokio::time::{Duration, Instant};

    use super::{CIMD_GLOBAL_PERMITS, CIMD_PER_SOURCE_PERMITS, CimdBulkhead, CimdBulkheadError};

    fn ip(octet: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, octet))
    }

    #[tokio::test(start_paused = true)]
    async fn try_acquire_respects_global_and_per_source_caps() {
        let bulkhead = CimdBulkhead::new();
        let first = bulkhead.try_acquire(ip(1)).unwrap();
        let second = bulkhead.try_acquire(ip(1)).unwrap();
        assert!(bulkhead.try_acquire(ip(1)).is_none());
        assert_eq!(CIMD_PER_SOURCE_PERMITS, 2);

        let mut held = vec![first, second];
        for octet in 2..=15 {
            held.push(bulkhead.try_acquire(ip(octet)).unwrap());
        }
        assert_eq!(held.len(), CIMD_GLOBAL_PERMITS);
        assert!(bulkhead.try_acquire(ip(16)).is_none());
        drop(held);
        assert!(bulkhead.try_acquire(ip(1)).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn drop_releases_immediately() {
        let bulkhead = CimdBulkhead::new();
        let permit = bulkhead.try_acquire(ip(1)).unwrap();
        drop(permit);
        assert!(bulkhead.try_acquire(ip(1)).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn zero_count_entries_are_removed() {
        let bulkhead = CimdBulkhead::new();
        for octet in 1..=CIMD_GLOBAL_PERMITS as u8 {
            let permit = bulkhead.try_acquire(ip(octet)).unwrap();
            drop(permit);
        }
        for octet in 17..=24 {
            assert!(
                bulkhead.try_acquire(ip(octet)).is_some(),
                "stale zero-count keys must not accumulate"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ipv4_mapped_ipv6_shares_the_per_source_bucket() {
        let bulkhead = CimdBulkhead::new();
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3));
        let v6: IpAddr = "::ffff:10.1.2.3".parse().unwrap();
        let first = bulkhead.try_acquire(v4).unwrap();
        let second = bulkhead.try_acquire(v6).unwrap();
        assert!(bulkhead.try_acquire(v4).is_none());
        drop(first);
        drop(second);
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_wakes_on_release_and_times_out() {
        let bulkhead = CimdBulkhead::new();
        let mut held = Vec::new();
        for octet in 1..=CIMD_GLOBAL_PERMITS as u8 {
            held.push(bulkhead.try_acquire(ip(octet)).unwrap());
        }

        let waiting = Arc::clone(&bulkhead);
        let deadline = Instant::now() + Duration::from_secs(5);
        let waiter = tokio::spawn(async move { waiting.acquire(ip(1), deadline).await });
        tokio::task::yield_now().await;
        drop(held.pop());
        drop(waiter.await.unwrap().expect("released permit wakes waiter"));
        drop(held);

        let mut held = Vec::new();
        for octet in 1..=CIMD_GLOBAL_PERMITS as u8 {
            held.push(bulkhead.try_acquire(ip(octet)).unwrap());
        }
        let waiting = Arc::clone(&bulkhead);
        let deadline = Instant::now() + Duration::from_secs(1);
        let waiter = tokio::spawn(async move { waiting.acquire(ip(9), deadline).await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(matches!(
            waiter.await.unwrap(),
            Err(CimdBulkheadError::Timeout)
        ));
        drop(held);
    }

    #[tokio::test(start_paused = true)]
    async fn aborted_task_still_releases_its_permit() {
        let bulkhead = CimdBulkhead::new();
        let permit = bulkhead.try_acquire(ip(1)).unwrap();
        let handle = tokio::spawn(async move {
            let _permit = permit;
            std::future::pending::<()>().await;
        });
        handle.abort();
        let _ = handle.await;
        assert!(bulkhead.try_acquire(ip(1)).is_some());
    }
}
