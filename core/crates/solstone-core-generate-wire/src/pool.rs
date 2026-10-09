// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-local attested confidential channel pool.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use solstone_core_spp_ratls::AttestedChannel;

/// Idle attested channels older than this are not reused.
///
/// The bound stays below the service's idle close (180s). It stays well below
/// the service's channel lifetime, which is checked when each request head
/// arrives. Reuse shrinks an in-flight request's headroom to the service's
/// hard deadline by at most this age. A reused channel is not probed again,
/// so content can reach the service for at most about 120 seconds after access ends.
pub const CONFIDENTIAL_CHANNEL_REUSE_MAX_AGE: Duration = Duration::from_secs(120);

// An offline-status channel is admitted with at least the request window plus
// a margin of signed status left, and its age is counted from that admission.
// Reusing it longer than the window would start requests the status no longer
// covers, so raising the reuse age must raise the admission headroom with it.
const _: () = assert!(
    CONFIDENTIAL_CHANNEL_REUSE_MAX_AGE.as_secs()
        <= solstone_core_spp_ratls::OFFLINE_STATUS_REQUEST_WINDOW.as_secs()
);

#[derive(Clone, PartialEq, Eq)]
pub struct RedactedCredential(pub String);

impl fmt::Debug for RedactedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PoolKey {
    pub journal_path: PathBuf,
    pub authority: String,
    pub credential: Option<RedactedCredential>,
}

pub trait PoolClock: Send + Sync {
    fn now_system(&self) -> SystemTime;
    fn now_monotonic(&self) -> Instant;
}

#[derive(Default)]
pub struct SystemPoolClock;

impl PoolClock for SystemPoolClock {
    fn now_system(&self) -> SystemTime {
        SystemTime::now()
    }
    fn now_monotonic(&self) -> Instant {
        Instant::now()
    }
}

pub struct IdleChannel {
    pub key: PoolKey,
    pub channel: AttestedChannel,
    pub created_at_system: SystemTime,
    pub created_at_monotonic: Instant,
    pub epoch: u64,
}

pub struct ConfidentialChannelPool {
    idle: Mutex<Vec<IdleChannel>>,
    in_use: AtomicUsize,
    epoch: AtomicU64,
    clock: Arc<dyn PoolClock>,
    max_in_flight: usize,
}

impl Default for ConfidentialChannelPool {
    fn default() -> Self {
        Self::new(1, Arc::new(SystemPoolClock))
    }
}

pub enum PoolAcquisition<'a> {
    Reused(PooledChannelGuard<'a>),
    FreshSlot(PooledChannelGuard<'a>, u64),
    CapacityExhausted,
}

impl ConfidentialChannelPool {
    pub fn new(max_in_flight: usize, clock: Arc<dyn PoolClock>) -> Self {
        Self {
            idle: Mutex::new(Vec::new()),
            in_use: AtomicUsize::new(0),
            epoch: AtomicU64::new(0),
            clock,
            max_in_flight: max_in_flight.max(1),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    pub fn clock(&self) -> &dyn PoolClock {
        &*self.clock
    }

    pub fn max_in_flight(&self) -> usize {
        self.max_in_flight
    }

    fn lock_idle(&self) -> MutexGuard<'_, Vec<IdleChannel>> {
        match self.idle.lock() {
            Ok(guard) => guard,
            Err(poison) => poison.into_inner(),
        }
    }

    fn sweep_idle_locked(&self, idle: &mut Vec<IdleChannel>) {
        let current_epoch = self.epoch.load(Ordering::Acquire);
        let now_sys = self.clock.now_system();
        let now_mono = self.clock.now_monotonic();

        idle.retain(|entry| {
            if entry.epoch != current_epoch {
                return false;
            }
            let Ok(sys_age) = now_sys.duration_since(entry.created_at_system) else {
                return false;
            };
            if sys_age > CONFIDENTIAL_CHANNEL_REUSE_MAX_AGE {
                return false;
            }
            let mono_age = now_mono.duration_since(entry.created_at_monotonic);
            if mono_age > CONFIDENTIAL_CHANNEL_REUSE_MAX_AGE {
                return false;
            }
            true
        });
    }

    /// Drop idle channels that are not `key`.
    ///
    /// Called only after this request has committed to `key`: a reused channel
    /// passed the alive check, or a fresh establishment succeeded. Below the
    /// limit, an unreachable dial leaves other idle channels in place; at the
    /// limit, `fresh_slot` has already evicted one to make room.
    pub fn drop_other_idle_keys(&self, key: &PoolKey) {
        let mut idle = self.lock_idle();
        self.sweep_idle_locked(&mut idle);
        idle.retain(|entry| entry.key == *key);
    }

    /// Reserves a slot for a channel this request will establish.
    ///
    /// Idle channels never refuse a fresh one: when live channels are at the
    /// limit, idle entries for other keys go first, then the oldest. Only
    /// in-use channels at the limit refuse, which a session's own worker bound
    /// never reaches.
    pub fn fresh_slot<'a>(&'a self, key: &PoolKey) -> Option<(PooledChannelGuard<'a>, u64)> {
        let mut idle = self.lock_idle();
        self.sweep_idle_locked(&mut idle);
        let in_use = self.in_use.load(Ordering::Relaxed);
        while !idle.is_empty() && idle.len() + in_use >= self.max_in_flight {
            let victim = idle
                .iter()
                .position(|entry| entry.key != *key)
                .unwrap_or_else(|| {
                    idle.iter()
                        .enumerate()
                        .min_by_key(|(_, entry)| entry.created_at_monotonic)
                        .map_or(0, |(index, _)| index)
                });
            idle.remove(victim);
        }
        if in_use >= self.max_in_flight {
            return None;
        }
        self.in_use.fetch_add(1, Ordering::Relaxed);
        let epoch = self.epoch.load(Ordering::Acquire);
        drop(idle);
        let guard = PooledChannelGuard {
            pool: self,
            key: key.clone(),
            channel: None,
            created_at_system: self.clock.now_system(),
            created_at_monotonic: self.clock.now_monotonic(),
            epoch,
            in_use_active: true,
            checked_out: false,
        };
        Some((guard, epoch))
    }

    pub fn checkout_or_slot<'a>(&'a self, key: &PoolKey) -> PoolAcquisition<'a> {
        let mut idle = self.lock_idle();
        self.sweep_idle_locked(&mut idle);

        if let Some(pos) = idle.iter().rposition(|entry| entry.key == *key) {
            let entry = idle.remove(pos);
            self.in_use.fetch_add(1, Ordering::Relaxed);
            drop(idle);
            return PoolAcquisition::Reused(PooledChannelGuard {
                pool: self,
                key: key.clone(),
                channel: Some(entry.channel),
                created_at_system: entry.created_at_system,
                created_at_monotonic: entry.created_at_monotonic,
                epoch: entry.epoch,
                in_use_active: true,
                checked_out: true,
            });
        }
        drop(idle);
        match self.fresh_slot(key) {
            Some((guard, epoch)) => PoolAcquisition::FreshSlot(guard, epoch),
            None => PoolAcquisition::CapacityExhausted,
        }
    }

    pub fn record_establishment_failed(&self) {
        self.epoch.fetch_add(1, Ordering::Release);
        self.drain_idle();
    }

    pub fn record_establishment_unreachable(&self) {
        // Unreachable: do not bump epoch, do not drain idle
    }

    pub fn drain_idle(&self) {
        let mut idle = self.lock_idle();
        idle.clear();
    }
}

pub struct PooledChannelGuard<'a> {
    pool: &'a ConfidentialChannelPool,
    key: PoolKey,
    channel: Option<AttestedChannel>,
    created_at_system: SystemTime,
    created_at_monotonic: Instant,
    epoch: u64,
    in_use_active: bool,
    pub checked_out: bool,
}

impl<'a> PooledChannelGuard<'a> {
    pub fn channel_mut(&mut self) -> Option<&mut AttestedChannel> {
        self.channel.as_mut()
    }

    pub fn set_established(
        &mut self,
        channel: AttestedChannel,
        created_at_system: SystemTime,
        created_at_monotonic: Instant,
        epoch: u64,
    ) {
        self.channel = Some(channel);
        self.created_at_system = created_at_system;
        self.created_at_monotonic = created_at_monotonic;
        self.epoch = epoch;
    }

    /// Returns the channel to the pool. This is the only way back: dropping the
    /// guard without releasing always discards the channel.
    pub fn release(mut self) {
        if self.in_use_active {
            self.pool.in_use.fetch_sub(1, Ordering::Relaxed);
            self.in_use_active = false;
        }
        if self.epoch != self.pool.epoch.load(Ordering::Acquire) {
            return;
        }
        let Some(mut channel) = self.channel.take() else {
            return;
        };
        if !channel.clean_to_reuse() {
            return;
        }
        let mut idle = self.pool.lock_idle();
        self.pool.sweep_idle_locked(&mut idle);
        let in_use = self.pool.in_use.load(Ordering::Relaxed);
        if idle.len() + in_use < self.pool.max_in_flight {
            idle.push(IdleChannel {
                key: self.key.clone(),
                channel,
                created_at_system: self.created_at_system,
                created_at_monotonic: self.created_at_monotonic,
                epoch: self.epoch,
            });
        }
    }
}

impl<'a> Drop for PooledChannelGuard<'a> {
    fn drop(&mut self) {
        if self.in_use_active {
            self.pool.in_use.fetch_sub(1, Ordering::Relaxed);
            self.in_use_active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockClock {
        system: Mutex<SystemTime>,
        monotonic: Mutex<Instant>,
    }

    impl MockClock {
        fn new() -> Self {
            Self {
                system: Mutex::new(SystemTime::now()),
                monotonic: Mutex::new(Instant::now()),
            }
        }

        #[allow(dead_code)]
        fn advance(&self, duration: Duration) {
            *self.system.lock().unwrap() += duration;
            *self.monotonic.lock().unwrap() += duration;
        }
    }

    impl PoolClock for MockClock {
        fn now_system(&self) -> SystemTime {
            *self.system.lock().unwrap()
        }
        fn now_monotonic(&self) -> Instant {
            *self.monotonic.lock().unwrap()
        }
    }

    #[test]
    fn empty_pool_checkout_returns_fresh_slot() {
        let clock = Arc::new(MockClock::new());
        let pool = ConfidentialChannelPool::new(1, clock);
        let key = PoolKey {
            journal_path: PathBuf::from("/test/journal"),
            authority: "127.0.0.1:9000".to_owned(),
            credential: None,
        };
        match pool.checkout_or_slot(&key) {
            PoolAcquisition::FreshSlot(guard, epoch) => {
                assert_eq!(epoch, 0);
                assert!(!guard.checked_out);
            }
            _ => panic!("expected fresh slot"),
        }
    }

    #[test]
    fn capacity_exhausted_when_max_in_flight_reached() {
        let clock = Arc::new(MockClock::new());
        let pool = ConfidentialChannelPool::new(1, clock);
        let key = PoolKey {
            journal_path: PathBuf::from("/test/journal"),
            authority: "127.0.0.1:9000".to_owned(),
            credential: None,
        };
        let _guard = match pool.checkout_or_slot(&key) {
            PoolAcquisition::FreshSlot(guard, _) => guard,
            _ => panic!("expected fresh slot"),
        };
        match pool.checkout_or_slot(&key) {
            PoolAcquisition::CapacityExhausted => {}
            _ => panic!("expected capacity exhausted"),
        }
    }

    #[test]
    fn record_establishment_failed_increments_epoch_and_drains() {
        let clock = Arc::new(MockClock::new());
        let pool = ConfidentialChannelPool::new(1, clock);
        assert_eq!(pool.epoch(), 0);
        pool.record_establishment_failed();
        assert_eq!(pool.epoch(), 1);
    }

    #[test]
    fn record_establishment_unreachable_does_not_bump_epoch() {
        let clock = Arc::new(MockClock::new());
        let pool = ConfidentialChannelPool::new(1, clock);
        assert_eq!(pool.epoch(), 0);
        pool.record_establishment_unreachable();
        assert_eq!(pool.epoch(), 0);
    }

    #[test]
    fn drain_idle_clears_pool() {
        let clock = Arc::new(MockClock::new());
        let pool = ConfidentialChannelPool::new(1, clock);
        pool.drain_idle();
    }
}
