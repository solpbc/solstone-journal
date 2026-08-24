// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;

use solstone_core_journal_io::is_day_key;

use crate::error::{ConvergenceError, Refusal};

pub(crate) const HEALTH: &str = "health";
pub(crate) const CHRONICLE: &str = "chronicle";
pub(crate) const STREAM_UPDATED: &str = "stream.updated";
pub(crate) const DAILY_UPDATED: &str = "daily.updated";
pub(crate) const CONVERGENCE: &str = "convergence";
pub(crate) const DAYS: &str = "days";
pub(crate) const RECORDS: &str = "records";
pub(crate) const CLAIM: &str = "claim";
pub(crate) const INTENTS: &str = "intents";
pub(crate) const ACTIVES: &str = "actives";
pub(crate) const TERMINALS: &str = "terminals";
pub(crate) const CLEARANCE: &str = "clearance";
pub(crate) const TOPOLOGY_LOCK: &str = "topology.lock";
pub(crate) const REGISTRY_LOCK: &str = "registry.lock";
pub(crate) const REGISTRY: &str = "registry";
pub(crate) const SECRET: &str = "secret.json";
pub(crate) const OWNERS: &str = "owners";
pub(crate) const LINKS: &str = "links";
pub(crate) const DECISIONS: &str = "decisions";
pub(crate) const GRANTS: &str = "grants";
pub(crate) const MEMBERS: &str = "members";
pub(crate) const BARRIERS: &str = "barriers";
pub(crate) const REVOCATIONS: &str = "revocations";
pub(crate) const TOMBSTONES: &str = "tombstones";
pub(crate) const ROOT_WITNESS: &str = "root.wit.json";
pub(crate) const ALLOCATOR: &str = "allocator.json";
pub(crate) const CLAIM_HEAD: &str = "head.json";

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

/// Non-empty unique day set for internal lock acquisition. Does not sort.
pub(crate) fn require_nonempty_unique(days: &[DayKey]) -> Result<(), ConvergenceError> {
    if days.is_empty() {
        return Err(ConvergenceError::Refused(Refusal::NonCanonicalDays));
    }
    let mut seen = std::collections::BTreeSet::new();
    for day in days {
        if !seen.insert(day) {
            return Err(ConvergenceError::Refused(Refusal::DuplicateDays));
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

pub(crate) fn claim_revision_name(revision: u64) -> OsString {
    OsString::from(format!("rev.{revision}.json"))
}

pub(crate) fn intent_name(serial: u64) -> OsString {
    OsString::from(format!("{serial}.json"))
}

pub(crate) fn active_name(serial: u64) -> OsString {
    OsString::from(format!("{serial}.json"))
}

pub(crate) fn terminal_name(serial: u64) -> OsString {
    OsString::from(format!("{serial}.json"))
}

pub(crate) fn member_name(day: &DayKey) -> OsString {
    OsString::from(format!("{}.clear.json", day.as_str()))
}

pub(crate) fn barrier_name(serial: u64) -> OsString {
    OsString::from(format!("{serial}.barrier.json"))
}

pub(crate) fn prepared_owner_name(operation_id: &str) -> OsString {
    OsString::from(format!("{operation_id}.json"))
}

pub(crate) const ACTIVE_BARRIER_SUFFIX: &str = "active";
pub(crate) const SUPERSEDED_BARRIER_SUFFIX: &str = "superseded";

pub(crate) fn decision_name(serial: u64) -> OsString {
    OsString::from(format!("{serial}.json"))
}

pub(crate) fn serial_dir(serial: u64) -> String {
    serial.to_string()
}

pub(crate) fn member_file_name(tuple: &crate::schema::GrantTuple) -> OsString {
    OsString::from(format!(
        "{}.{}.{}.json",
        tuple.day,
        tuple.writer_family.as_str(),
        tuple.target_scope.as_str()
    ))
}

pub(crate) fn barrier_file_name(serial: u64, suffix: &str) -> OsString {
    OsString::from(format!("{serial}.{suffix}.json"))
}

/// The initial owner-intent link is directly addressed before an intent serial
/// exists.  Both inputs are canonical fixed-width digests, so this name is a
/// bounded descriptor-relative lookup rather than a namespace traversal.
pub(crate) fn link_name(owner_binding_digest: &str, selector_digest: &str) -> OsString {
    OsString::from(format!("{owner_binding_digest}.{selector_digest}.json"))
}

/// A later-dirty successor keeps the initial link immutable and is addressed
/// by that same stable identity plus its known serial.
pub(crate) fn successor_link_name(
    owner_binding_digest: &str,
    selector_digest: &str,
    serial: u64,
) -> OsString {
    OsString::from(format!(
        "{owner_binding_digest}.{selector_digest}.{serial}.json"
    ))
}

pub(crate) fn consumption_witness_name(day: &DayKey, serial: u64) -> OsString {
    OsString::from(format!("{}.consumed.{serial}.wit.json", day.as_str()))
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;

    #[test]
    fn require_nonempty_unique_refuses_empty_and_duplicates() {
        assert!(require_nonempty_unique(&[]).is_err());
        let a = DayKey::parse("20260823").unwrap();
        let b = DayKey::parse("20260824").unwrap();
        assert!(require_nonempty_unique(&[a.clone(), a.clone()]).is_err());
        assert!(require_nonempty_unique(&[b.clone(), a.clone()]).is_ok());
        assert!(require_nonempty_unique(&[a, b]).is_ok());
    }

    #[test]
    fn day_key_parse_refuses_alias() {
        let error = DayKey::parse("2026-08-23").unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::NonCanonicalDays)
        ));
    }
}
