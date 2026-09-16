// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reentrant, journal-wide facet mutation locking.

pub use solstone_core_entity::{
    FacetTrustLock, FacetTrustLockError, hold_facet_trust_lock, hold_facet_trust_lock_raw_for_test,
};
