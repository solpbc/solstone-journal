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

    #[cfg(all(test, not(feature = "full-tests")))]
    pub(crate) fn debug_state(&self) -> (usize, usize) {
        let state = lock(&self.inner);
        (state.global, state.per_source.len())
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    pub(crate) fn debug_held(&self, source: IpAddr) -> usize {
        let source = canonicalize_ip(source);
        lock(&self.inner)
            .per_source
            .get(&source)
            .copied()
            .unwrap_or(0)
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

    #[tokio::test(start_paused = true)]
    async fn ten_thousand_sources_return_to_empty_bookkeeping() {
        let bulkhead = CimdBulkhead::new();
        let twin = IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3));
        let twin_mapped: IpAddr = "::ffff:10.1.2.3".parse().unwrap();
        for index in 1_u32..=10_000 {
            let source = IpAddr::V4(Ipv4Addr::from(index));
            match index % 5 {
                0 => {
                    let mut held = Vec::new();
                    for offset in 0..CIMD_GLOBAL_PERMITS as u32 {
                        let filler = IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + offset));
                        held.push(bulkhead.try_acquire(filler).unwrap());
                    }
                    assert!(bulkhead.try_acquire(source).is_none());
                    drop(held);
                }
                1 => {
                    let first = bulkhead.try_acquire(source).unwrap();
                    let second = bulkhead.try_acquire(source).unwrap();
                    let waiting = Arc::clone(&bulkhead);
                    let deadline = Instant::now() + Duration::from_secs(30);
                    let mut waiter =
                        std::pin::pin!(async move { waiting.acquire(source, deadline).await });
                    tokio::select! {
                        biased;
                        _ = &mut waiter => panic!("cancel waiter resolved"),
                        () = tokio::task::yield_now() => {}
                    }
                    drop(waiter);
                    drop(first);
                    drop(second);
                }
                _ => {
                    let permit = bulkhead.try_acquire(source).unwrap();
                    if index == 42 {
                        let extra = bulkhead.try_acquire(twin).unwrap();
                        let extra_mapped = bulkhead.try_acquire(twin_mapped).unwrap();
                        assert!(bulkhead.try_acquire(twin).is_none());
                        drop(extra);
                        drop(extra_mapped);
                    }
                    drop(permit);
                }
            }
        }
        assert_eq!(bulkhead.debug_state(), (0, 0));
    }

    async fn wait_until_parked(bulkhead: &Arc<CimdBulkhead>, source: IpAddr, held: usize) {
        for _ in 0..32 {
            if bulkhead.debug_held(source) == held {
                tokio::task::yield_now().await;
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("waiter did not observe held={held}");
    }

    #[tokio::test(start_paused = true)]
    async fn same_source_final_release_is_deterministic_across_outcomes() {
        let source = ip(7);
        // Success: waiter lands on the last per-source slot freeing.
        {
            let bulkhead = CimdBulkhead::new();
            let first = bulkhead.try_acquire(source).unwrap();
            let second = bulkhead.try_acquire(source).unwrap();
            assert_eq!(bulkhead.debug_held(source), 2);
            assert_eq!(bulkhead.debug_state().1, 1);
            let waiting = Arc::clone(&bulkhead);
            let deadline = Instant::now() + Duration::from_secs(30);
            let waiter = tokio::spawn(async move { waiting.acquire(source, deadline).await });
            wait_until_parked(&bulkhead, source, 2).await;
            drop(first);
            let acquired = waiter.await.unwrap().expect("success waiter acquires");
            assert!(bulkhead.debug_held(source) <= CIMD_PER_SOURCE_PERMITS);
            assert_eq!(bulkhead.debug_state().1, 1);
            drop(second);
            drop(acquired);
            assert_eq!(bulkhead.debug_held(source), 0);
            assert_eq!(bulkhead.debug_state(), (0, 0));
        }
        // Refusal: try_acquire at cap never inserts a third slot.
        {
            let bulkhead = CimdBulkhead::new();
            let first = bulkhead.try_acquire(source).unwrap();
            let second = bulkhead.try_acquire(source).unwrap();
            assert!(bulkhead.try_acquire(source).is_none());
            assert_eq!(bulkhead.debug_held(source), 2);
            assert_eq!(bulkhead.debug_state().1, 1);
            drop(first);
            drop(second);
            assert_eq!(bulkhead.debug_state(), (0, 0));
        }
        // Cancel: drop the waiting acquire future, then release the last permits.
        {
            let bulkhead = CimdBulkhead::new();
            let first = bulkhead.try_acquire(source).unwrap();
            let second = bulkhead.try_acquire(source).unwrap();
            let waiting = Arc::clone(&bulkhead);
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut waiter = std::pin::pin!(async move { waiting.acquire(source, deadline).await });
            tokio::select! {
                biased;
                _ = &mut waiter => panic!("cancel waiter resolved"),
                () = tokio::task::yield_now() => {}
            }
            drop(waiter);
            drop(first);
            drop(second);
            assert_eq!(bulkhead.debug_state(), (0, 0));
        }
        // Timeout: waiter expires, last permits still drop the map entry.
        {
            let bulkhead = CimdBulkhead::new();
            let first = bulkhead.try_acquire(source).unwrap();
            let second = bulkhead.try_acquire(source).unwrap();
            let waiting = Arc::clone(&bulkhead);
            let deadline = Instant::now() + Duration::from_secs(1);
            let waiter = tokio::spawn(async move { waiting.acquire(source, deadline).await });
            wait_until_parked(&bulkhead, source, 2).await;
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(matches!(
                waiter.await.unwrap(),
                Err(CimdBulkheadError::Timeout)
            ));
            drop(first);
            drop(second);
            assert_eq!(bulkhead.debug_state(), (0, 0));
        }
        // Task failure: panicked holder still releases on drop.
        {
            let bulkhead = CimdBulkhead::new();
            let permit = bulkhead.try_acquire(source).unwrap();
            let handle = tokio::spawn(async move {
                let _permit = permit;
                panic!("forced holder failure");
            });
            assert!(handle.await.unwrap_err().is_panic());
            assert_eq!(bulkhead.debug_state(), (0, 0));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn distinct_sources_progress_concurrently_under_global_capacity() {
        let bulkhead = CimdBulkhead::new();
        let left = ip(1);
        let right = ip(2);
        let left_bulkhead = Arc::clone(&bulkhead);
        let right_bulkhead = Arc::clone(&bulkhead);
        let (left_permit, right_permit) = tokio::join!(
            async move {
                left_bulkhead
                    .acquire(left, Instant::now() + Duration::from_secs(5))
                    .await
                    .unwrap()
            },
            async move {
                right_bulkhead
                    .acquire(right, Instant::now() + Duration::from_secs(5))
                    .await
                    .unwrap()
            },
        );
        assert_eq!(bulkhead.debug_held(left), 1);
        assert_eq!(bulkhead.debug_held(right), 1);
        assert_eq!(bulkhead.debug_state(), (2, 2));
        let left_second = bulkhead.try_acquire(left).unwrap();
        let right_second = bulkhead.try_acquire(right).unwrap();
        assert_eq!(bulkhead.debug_held(left), 2);
        assert_eq!(bulkhead.debug_held(right), 2);
        drop(left_permit);
        drop(right_permit);
        drop(left_second);
        drop(right_second);
        assert_eq!(bulkhead.debug_state(), (0, 0));
    }
}
