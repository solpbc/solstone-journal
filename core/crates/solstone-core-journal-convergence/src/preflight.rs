// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::time::Duration;

use solstone_core_journal_io::JournalRoot;

use crate::digest::RecordDigest;
use crate::error::{ConvergenceError, Refusal};
use crate::init::initialize;
use crate::layout::DayKey;
use crate::lock::LOCK_TIMEOUT;
use crate::schema::day_set_subdigest;
use crate::store::ConvergenceStore;

/// Pure canonicalization of caller day strings. Performs no I/O.
pub fn preflight<I, S>(days: I) -> Result<Preflight, ConvergenceError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parsed = Vec::new();
    let mut seen = BTreeSet::new();
    for day in days {
        let key = DayKey::parse(day.as_ref())?;
        if !seen.insert(key.clone()) {
            return Err(ConvergenceError::Refused(Refusal::DuplicateDays));
        }
        parsed.push(key);
    }
    if parsed.is_empty() {
        return Ok(Preflight::Empty);
    }
    parsed.sort();
    Ok(Preflight::Ready(CanonicalDaySet { days: parsed }))
}

/// Discovery canonicalize: empty is a refusal, not a no-op (AC3).
#[allow(dead_code)]
pub(crate) fn canonicalize_discovered<I, S>(days: I) -> Result<CanonicalDaySet, ConvergenceError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    match preflight(days)? {
        Preflight::Empty => Err(ConvergenceError::Refused(Refusal::NonCanonicalDays)),
        Preflight::Ready(set) => Ok(set),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Preflight {
    Empty,
    Ready(CanonicalDaySet),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalDaySet {
    days: Vec<DayKey>,
}

impl CanonicalDaySet {
    pub fn days(&self) -> &[DayKey] {
        &self.days
    }

    pub fn subdigest(&self) -> Result<RecordDigest, ConvergenceError> {
        day_set_subdigest(&self.days)
    }

    /// Idempotent init-or-revalidate under `topology.lock` with no day locks.
    pub fn admit(self, root: JournalRoot) -> Result<Admitted, ConvergenceError> {
        match initialize(&root) {
            Ok(()) => {}
            Err(ConvergenceError::Refused(Refusal::AlreadyInitialized)) => {}
            Err(error) => return Err(error),
        }
        let store = ConvergenceStore::open(root)?;
        Ok(Admitted {
            store,
            days: self,
            lock_timeout: LOCK_TIMEOUT,
        })
    }
}

/// Initialized retained root plus canonical day set. Not `Clone`.
pub struct Admitted {
    pub(crate) store: ConvergenceStore,
    pub(crate) days: CanonicalDaySet,
    pub(crate) lock_timeout: Duration,
}

impl Admitted {
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    pub(crate) fn store(&self) -> &ConvergenceStore {
        &self.store
    }

    pub fn days(&self) -> &[DayKey] {
        self.days.days()
    }

    pub(crate) fn lock_timeout(&self) -> Duration {
        self.lock_timeout
    }
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, open_root, snapshot_tree};

    #[test]
    fn ac10_10_5_empty_is_typed_noop() {
        let temporary = TempDir::new("empty-preflight");
        let (journal, _root) = open_root(&temporary);
        let before = snapshot_tree(&journal);
        let outcome = preflight::<[&str; 0], &str>([]).unwrap();
        assert!(matches!(outcome, Preflight::Empty));
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn ac10_10_6_alias_refuses_without_io() {
        let temporary = TempDir::new("alias-preflight");
        let (journal, _root) = open_root(&temporary);
        let before = snapshot_tree(&journal);
        let error = preflight(["2026-08-23"]).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::NonCanonicalDays)
        ));
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn ac10_10_7_duplicate_refuses_without_io() {
        let temporary = TempDir::new("dup-preflight");
        let (journal, _root) = open_root(&temporary);
        let before = snapshot_tree(&journal);
        let error = preflight(["20260823", "20260823"]).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::DuplicateDays)
        ));
        assert_eq!(before, snapshot_tree(&journal));
    }

    #[test]
    fn ac10_10_8_permuted_inputs_same_encoding() {
        let left = match preflight(["20260824", "20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("non-empty"),
        };
        let right = match preflight(["20260823", "20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("non-empty"),
        };
        assert_eq!(left.days(), right.days());
        assert_eq!(left.subdigest().unwrap(), right.subdigest().unwrap());
    }
}
