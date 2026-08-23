// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;

use solstone_core_journal_io::is_day_key;

use crate::error::{ConvergenceError, Refusal};

pub(crate) const HEALTH: &str = "health";
pub(crate) const CONVERGENCE: &str = "convergence";
pub(crate) const DAYS: &str = "days";
pub(crate) const RECORDS: &str = "records";
pub(crate) const TOPOLOGY_LOCK: &str = "topology.lock";
pub(crate) const ROOT_WITNESS: &str = "root.wit.json";
pub(crate) const ALLOCATOR: &str = "allocator.json";

/// Canonical 8-digit chronicle day.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DayKey(String);

impl DayKey {
    pub fn parse(value: &str) -> Result<Self, ConvergenceError> {
        if !is_day_key(value) {
            return Err(ConvergenceError::Refused(Refusal::NonCanonicalDays));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Public canonical validation. Refuses empty, duplicates, and unsorted input.
pub fn validate_day_set(days: &[DayKey]) -> Result<(), ConvergenceError> {
    if days.is_empty() {
        return Err(ConvergenceError::Refused(Refusal::NonCanonicalDays));
    }
    for window in days.windows(2) {
        if window[0] >= window[1] {
            return Err(ConvergenceError::Refused(Refusal::NonCanonicalDays));
        }
    }
    Ok(())
}

pub(crate) fn day_lock_name(day: &DayKey) -> OsString {
    OsString::from(format!("{}.lock", day.as_str()))
}

pub(crate) fn adoption_name(day: &DayKey) -> OsString {
    OsString::from(format!("{}.adopt.json", day.as_str()))
}

pub(crate) fn ever_name(day: &DayKey) -> OsString {
    OsString::from(format!("{}.ever.wit.json", day.as_str()))
}

pub(crate) fn head_name(day: &DayKey) -> OsString {
    OsString::from(format!("{}.head.json", day.as_str()))
}

pub(crate) fn revision_witness_name(day: &DayKey, revision: u64) -> OsString {
    OsString::from(format!("{}.rev.{revision}.wit.json", day.as_str()))
}

pub(crate) fn record_file_name() -> &'static str {
    "record.json"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_day_set_refuses_empty_unsorted_and_duplicates() {
        assert!(validate_day_set(&[]).is_err());
        let a = DayKey::parse("20260823").unwrap();
        let b = DayKey::parse("20260824").unwrap();
        assert!(validate_day_set(&[b.clone(), a.clone()]).is_err());
        assert!(validate_day_set(&[a.clone(), a.clone()]).is_err());
        assert!(validate_day_set(&[a, b]).is_ok());
    }

    #[test]
    fn acquire_days_does_not_normalize() {
        let later = DayKey::parse("20260824").unwrap();
        let earlier = DayKey::parse("20260823").unwrap();
        let error = validate_day_set(&[later, earlier]).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::NonCanonicalDays)
        ));
    }
}
