// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-memory pairing-attempt limiter keyed by source and generation.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use super::cimd::canonicalize_ip;

const MAX_PAIRING_LIMIT_ENTRIES: usize = 4096;
const PAIRING_LIMIT_TTL: Duration = Duration::from_secs(600);
const MAX_PAIRING_FAILURES: u8 = 20;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PairingLimitKey {
    source: IpAddr,
    generation: u64,
}

struct PairingLimitEntry {
    failures: u8,
    expires_at: Instant,
}

/// Outcome of recording one pairing-code failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingFailureRecord {
    /// The failure was counted and the key is still under the 20-cap.
    Counted,
    /// This call was the 20th failure and just tripped the cap.
    JustTripped,
    /// The key was already over the cap, or a new key was refused at the bound.
    Limited,
}

/// Process-local pairing-attempt limiter.
pub(crate) struct PairingRateLimiter {
    inner: Mutex<HashMap<PairingLimitKey, PairingLimitEntry>>,
}

impl PairingRateLimiter {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn is_limited(&self, source: IpAddr, generation: u64) -> bool {
        let key = key(source, generation);
        let mut entries = lock(&self.inner);
        prune_expired(&mut entries, Instant::now());
        match entries.get(&key) {
            Some(entry) => entry.failures >= MAX_PAIRING_FAILURES,
            None => entries.len() >= MAX_PAIRING_LIMIT_ENTRIES,
        }
    }

    pub(crate) fn record_failure(&self, source: IpAddr, generation: u64) -> PairingFailureRecord {
        let key = key(source, generation);
        let now = Instant::now();
        let mut entries = lock(&self.inner);
        prune_expired(&mut entries, now);
        if let Some(entry) = entries.get_mut(&key) {
            if entry.failures >= MAX_PAIRING_FAILURES {
                return PairingFailureRecord::Limited;
            }
            entry.failures = entry.failures.saturating_add(1);
            if entry.failures >= MAX_PAIRING_FAILURES {
                PairingFailureRecord::JustTripped
            } else {
                PairingFailureRecord::Counted
            }
        } else if entries.len() >= MAX_PAIRING_LIMIT_ENTRIES {
            PairingFailureRecord::Limited
        } else {
            entries.insert(
                key,
                PairingLimitEntry {
                    failures: 1,
                    expires_at: now + PAIRING_LIMIT_TTL,
                },
            );
            PairingFailureRecord::Counted
        }
    }

    pub(crate) fn prune_generation(&self, generation: u64) {
        let mut entries = lock(&self.inner);
        prune_expired(&mut entries, Instant::now());
        entries.retain(|key, _| key.generation == generation);
    }
}

fn key(source: IpAddr, generation: u64) -> PairingLimitKey {
    PairingLimitKey {
        source: canonicalize_ip(source),
        generation,
    }
}

fn lock(
    mutex: &Mutex<HashMap<PairingLimitKey, PairingLimitEntry>>,
) -> std::sync::MutexGuard<'_, HashMap<PairingLimitKey, PairingLimitEntry>> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn prune_expired(entries: &mut HashMap<PairingLimitKey, PairingLimitEntry>, now: Instant) {
    entries.retain(|_, entry| entry.expires_at > now);
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::{
        MAX_PAIRING_FAILURES, MAX_PAIRING_LIMIT_ENTRIES, PAIRING_LIMIT_TTL, PairingFailureRecord,
        PairingRateLimiter,
    };

    fn ip(octet: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, octet))
    }

    #[tokio::test(start_paused = true)]
    async fn twentieth_failure_trips_and_further_calls_stay_limited() {
        let limiter = PairingRateLimiter::new();
        let source = ip(1);
        for _ in 0..19 {
            assert_eq!(
                limiter.record_failure(source, 1),
                PairingFailureRecord::Counted
            );
            assert!(!limiter.is_limited(source, 1));
        }
        assert_eq!(
            limiter.record_failure(source, 1),
            PairingFailureRecord::JustTripped
        );
        assert!(limiter.is_limited(source, 1));
        assert_eq!(
            limiter.record_failure(source, 1),
            PairingFailureRecord::Limited
        );
        assert_eq!(MAX_PAIRING_FAILURES, 20);
    }

    #[tokio::test(start_paused = true)]
    async fn ttl_expiry_starts_a_fresh_count() {
        let limiter = PairingRateLimiter::new();
        let source = ip(2);
        limiter.record_failure(source, 1);
        tokio::time::advance(PAIRING_LIMIT_TTL).await;
        assert!(!limiter.is_limited(source, 1));
        assert_eq!(
            limiter.record_failure(source, 1),
            PairingFailureRecord::Counted
        );
    }

    #[tokio::test(start_paused = true)]
    async fn generations_are_isolated_and_prune_keeps_current() {
        let limiter = PairingRateLimiter::new();
        let source = ip(3);
        for _ in 0..20 {
            limiter.record_failure(source, 1);
        }
        assert!(limiter.is_limited(source, 1));
        assert!(!limiter.is_limited(source, 2));
        assert_eq!(
            limiter.record_failure(source, 2),
            PairingFailureRecord::Counted
        );

        limiter.record_failure(ip(4), 1);
        limiter.record_failure(ip(5), 2);
        limiter.prune_generation(2);
        assert!(!limiter.is_limited(source, 1));
        assert!(!limiter.is_limited(ip(4), 1));
        assert_eq!(
            limiter.record_failure(ip(5), 2),
            PairingFailureRecord::Counted
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ipv4_mapped_ipv6_shares_the_v4_bucket() {
        let limiter = PairingRateLimiter::new();
        let v4 = IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3));
        let v6: IpAddr = "::ffff:10.1.2.3".parse().unwrap();
        for _ in 0..19 {
            limiter.record_failure(v4, 7);
        }
        assert_eq!(
            limiter.record_failure(v6, 7),
            PairingFailureRecord::JustTripped
        );
        assert!(limiter.is_limited(v4, 7));
    }

    #[tokio::test(start_paused = true)]
    async fn new_keys_are_rejected_at_the_entry_bound() {
        let limiter = PairingRateLimiter::new();
        for index in 0..MAX_PAIRING_LIMIT_ENTRIES {
            let source = IpAddr::V4(Ipv4Addr::from(index as u32));
            assert_eq!(
                limiter.record_failure(source, 1),
                PairingFailureRecord::Counted
            );
        }
        let overflow = IpAddr::V4(Ipv4Addr::from(MAX_PAIRING_LIMIT_ENTRIES as u32));
        assert!(limiter.is_limited(overflow, 1));
        assert_eq!(
            limiter.record_failure(overflow, 1),
            PairingFailureRecord::Limited
        );
        assert_eq!(
            limiter.record_failure(IpAddr::V4(Ipv4Addr::from(0)), 1),
            PairingFailureRecord::Counted
        );
    }
}
