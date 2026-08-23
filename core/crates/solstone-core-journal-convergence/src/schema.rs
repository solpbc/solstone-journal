// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::fd::AsFd;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    BoundAtomicOutcome, atomic_replace_bound, read_bytes_bound, write_bytes_exclusive_bound,
};

use crate::digest::{RecordDigest, canonical_json_bytes, digest_bytes, digest_value};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::layout::DayKey;

pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const ROLE_CLAIM_GENESIS: &str = "solstone.convergence.claim-genesis.v1";
pub(crate) const ROLE_CLAIM_HEAD: &str = "solstone.convergence.claim-head.v1";
pub(crate) const ROLE_CLAIM_REVISION: &str = "solstone.convergence.claim-revision.v1";
pub(crate) const ROLE_DAY_SET: &str = "solstone.convergence.day-set.v1";
pub(crate) const ROLE_OWNER_BINDING: &str = "solstone.convergence.owner-binding.v1";
pub(crate) const ROLE_VIRGIN: &str = "solstone.convergence.virgin.v1";
pub(crate) const ROLE_STREAM_UPDATED: &str = "solstone.convergence.stream-updated.v1";
pub(crate) const ROLE_INTENT: &str = "solstone.convergence.intent.v1";
pub(crate) const ROLE_ACTIVE: &str = "solstone.convergence.active.v1";
pub(crate) const OPERATION_ADVANCE_DIRTY: &str = "advance_dirty";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RootWitness {
    pub schema_version: u32,
    pub journal_id: String,
    pub auxiliary_time: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Allocator {
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub next_serial: u64,
    pub exhausted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Adoption {
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub adoption_id: String,
    pub day: String,
    pub auxiliary_time: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct EverWitness {
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub adoption_id: String,
    pub day: String,
    pub first_transition_serial: u64,
    pub dirty_generation: u64,
    pub completed_generation: u64,
    pub record_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionWitness {
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub adoption_id: String,
    pub day: String,
    pub record_revision: u64,
    pub first_transition_serial: u64,
    pub dirty_by_transition_serial: u64,
    pub dirty_generation: u64,
    pub completed_generation: u64,
    pub record_digest: String,
    pub prior_witness_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Head {
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub adoption_id: String,
    pub day: String,
    pub record_revision: u64,
    pub witness_digest: String,
    pub record_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DayRecord {
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub adoption_id: String,
    pub day: String,
    pub record_revision: u64,
    pub first_transition_serial: u64,
    pub dirty_by_transition_serial: u64,
    pub dirty_generation: u64,
    pub completed_generation: u64,
    pub auxiliary_time: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaimHead {
    pub role: String,
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub revision: u64,
    pub revision_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) enum ClaimTransition {
    Introduce,
    Release,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TableEntry {
    pub serial: u64,
    pub owner_binding_digest: String,
    pub intent_digest: String,
    pub introduced_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaimRevision {
    pub role: String,
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub revision: u64,
    pub prior_revision: u64,
    pub prior_revision_digest: String,
    pub transition: ClaimTransition,
    pub serial: u64,
    pub owner_binding_digest: String,
    pub day_set: Vec<String>,
    pub day_set_subdigest: String,
    pub intent_digest: String,
    pub table: BTreeMap<String, TableEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub(crate) enum Predecessor {
    Virgin {
        digest: String,
    },
    Member {
        member_digest: String,
        barrier_digest: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub(crate) enum PresentAbsent {
    Absent,
    Present { bytes: String, digest: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectionBinding {
    pub prior_stream: PresentAbsent,
    pub prior_daily: PresentAbsent,
    pub proposed_stream: PresentAbsent,
    pub proposed_daily: PresentAbsent,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    pub role: String,
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub serial: u64,
    pub operation: String,
    pub day_set: Vec<String>,
    pub day_set_subdigest: String,
    pub owner_binding_digest: String,
    pub claim_revision: u64,
    pub prior_claim_head_revision: u64,
    pub prior_claim_head_digest: String,
    pub prior_day_revisions: BTreeMap<String, u64>,
    pub proposed_day_revisions: BTreeMap<String, u64>,
    pub proposed_dirty_generations: BTreeMap<String, u64>,
    pub predecessors: BTreeMap<String, Predecessor>,
    pub projections: BTreeMap<String, ProjectionBinding>,
    pub intent_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Active {
    pub role: String,
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub serial: u64,
    pub owner_binding_digest: String,
    pub intent_digest: String,
    pub day_set: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct VirginProof {
    pub role: String,
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub adoption_id: String,
    pub day: String,
}

/// Canonical `stream.updated` payload.
///
/// Deliberately omits `intent_digest`. The final intent hashes this marker's
/// exact bytes and digest (`Present { bytes, digest }`); putting the intent
/// digest on the marker would cycle the digest graph.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct StreamUpdated {
    pub role: String,
    pub schema_version: u32,
    pub journal_id: String,
    pub root_id: String,
    pub adoption_id: String,
    pub day: String,
    pub dirty_generation: u64,
    pub author_serial: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DaySetCanon {
    pub role: String,
    pub days: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerBindingCanon {
    pub role: String,
    pub journal_id: String,
    pub root_id: String,
    pub owner_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaimGenesis {
    pub role: String,
    pub journal_id: String,
    pub root_id: String,
}

pub(crate) fn validate_record_numbers(record: &DayRecord) -> Result<(), ConvergenceError> {
    if record.schema_version != SCHEMA_VERSION {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Record,
        });
    }
    if record.record_revision == 0 {
        return Err(ConvergenceError::Refused(Refusal::PersistedZeroRevision));
    }
    if record.dirty_generation == 0 {
        return Err(ConvergenceError::Refused(
            Refusal::PersistedZeroDirtyGeneration,
        ));
    }
    if record.first_transition_serial == 0 || record.dirty_by_transition_serial == 0 {
        return Err(ConvergenceError::Refused(Refusal::PersistedZeroSerial));
    }
    if record.completed_generation > record.dirty_generation {
        return Err(ConvergenceError::Refused(Refusal::CompletedExceedsDirty));
    }
    Ok(())
}

pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(
    directory: &impl AsFd,
    name: &OsStr,
    role: DurableRole,
) -> Result<Option<T>, ConvergenceError> {
    let Some(bytes) = read_bytes_bound(directory, name).map_err(|error| map_read(role, error))?
    else {
        return Ok(None);
    };
    parse_json(&bytes, role).map(Some)
}

pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    role: DurableRole,
) -> Result<T, ConvergenceError> {
    let trimmed = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    serde_json::from_slice(trimmed).map_err(|error| map_serde(role, error))
}

pub(crate) fn write_json_exclusive<T: Serialize>(
    directory: &impl AsFd,
    name: &OsStr,
    value: &T,
    role: DurableRole,
) -> Result<RecordDigest, ConvergenceError> {
    let (digest, disk) = encode_disk(value)?;
    write_bytes_exclusive_bound(directory, name, &disk, 0o600).map_err(|error| {
        ConvergenceError::PreservedPrior {
            operation: "exclusive create",
            source: match error {
                solstone_core_journal_io::AtomicWriteError::Io { source, .. } => source,
            },
        }
    })?;
    let _ = role;
    Ok(digest)
}

pub(crate) fn replace_json<T: Serialize>(
    directory: &impl AsFd,
    name: &OsStr,
    value: &T,
) -> Result<(RecordDigest, BoundAtomicOutcome), ConvergenceError> {
    let (digest, disk) = encode_disk(value)?;
    let outcome = atomic_replace_bound(directory, name, &disk, 0o600).map_err(|error| {
        ConvergenceError::PreservedPrior {
            operation: error.operation,
            source: error.source,
        }
    })?;
    Ok((digest, outcome))
}

fn encode_disk<T: Serialize>(value: &T) -> Result<(RecordDigest, Vec<u8>), ConvergenceError> {
    let canonical = canonical_json_bytes(value)?;
    let digest = digest_bytes(&canonical);
    let mut disk = canonical;
    disk.push(b'\n');
    Ok((digest, disk))
}

pub(crate) fn record_digest(record: &DayRecord) -> Result<RecordDigest, ConvergenceError> {
    digest_value(record)
}

pub(crate) fn intent_digest(intent: &Intent) -> Result<RecordDigest, ConvergenceError> {
    crate::digest::digest_value_excluding(intent, "intent_digest")
}

pub(crate) fn day_set_subdigest(
    days: &[crate::layout::DayKey],
) -> Result<RecordDigest, ConvergenceError> {
    digest_value(&DaySetCanon {
        role: ROLE_DAY_SET.to_owned(),
        days: days.iter().map(|day| day.as_str().to_owned()).collect(),
    })
}

pub(crate) fn genesis_claim_digest(
    journal_id: &str,
    root_id: &str,
) -> Result<RecordDigest, ConvergenceError> {
    digest_value(&ClaimGenesis {
        role: ROLE_CLAIM_GENESIS.to_owned(),
        journal_id: journal_id.to_owned(),
        root_id: root_id.to_owned(),
    })
}

pub(crate) fn require_ids(
    journal_id: &str,
    root_id: &str,
    observed_journal: &str,
    observed_root: &str,
) -> Result<(), ConvergenceError> {
    if journal_id != observed_journal || root_id != observed_root {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Allocator,
        });
    }
    Ok(())
}

pub(crate) fn require_day(expected: &DayKey, observed: &str) -> Result<(), ConvergenceError> {
    if expected.as_str() != observed {
        return Err(ConvergenceError::Refused(Refusal::WrongDay {
            expected: expected.as_str().to_owned(),
            observed: observed.to_owned(),
        }));
    }
    Ok(())
}

fn map_read(role: DurableRole, error: solstone_core_journal_io::ReadError) -> ConvergenceError {
    match error {
        solstone_core_journal_io::ReadError::Io { source, .. } => ConvergenceError::Io {
            operation: "read bound json",
            role,
            source,
        },
        solstone_core_journal_io::ReadError::Malformed(_) => ConvergenceError::Unknown { role },
    }
}

fn map_serde(role: DurableRole, error: serde_json::Error) -> ConvergenceError {
    let message = error.to_string();
    if let Some(field) = unknown_field_name(&message) {
        return ConvergenceError::Refused(Refusal::UnknownField { field });
    }
    if message.contains("missing field")
        && (message.contains("serial") || message.contains("transition"))
    {
        return ConvergenceError::Refused(Refusal::MissingSerial);
    }
    ConvergenceError::Unknown { role }
}

fn unknown_field_name(message: &str) -> Option<String> {
    let rest = message.strip_prefix("unknown field `")?;
    let field = rest.split('`').next()?;
    Some(field.to_owned())
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;

    fn sample_record(revision: u64, dirty: u64, completed: u64) -> DayRecord {
        DayRecord {
            schema_version: 1,
            journal_id: "j".into(),
            root_id: "r".into(),
            adoption_id: "a".into(),
            day: "20260823".into(),
            record_revision: revision,
            first_transition_serial: 1,
            dirty_by_transition_serial: 1,
            dirty_generation: dirty,
            completed_generation: completed,
            auxiliary_time: "2026-08-23T00:00:00Z".into(),
        }
    }

    #[test]
    fn persisted_zero_revision_refused() {
        let error = validate_record_numbers(&sample_record(0, 1, 0)).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::PersistedZeroRevision)
        ));
    }

    #[test]
    fn persisted_zero_dirty_generation_refused() {
        let error = validate_record_numbers(&sample_record(1, 0, 0)).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::PersistedZeroDirtyGeneration)
        ));
    }

    #[test]
    fn completed_exceeds_dirty_refused() {
        let error = validate_record_numbers(&sample_record(1, 1, 2)).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::CompletedExceedsDirty)
        ));
    }

    #[test]
    fn persisted_zero_serial_refused() {
        let mut record = sample_record(1, 1, 0);
        record.first_transition_serial = 0;
        let error = validate_record_numbers(&record).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::PersistedZeroSerial)
        ));
    }

    #[test]
    fn unknown_field_is_refused() {
        let error = parse_json::<DayRecord>(
            br#"{"schema_version":1,"journal_id":"j","root_id":"r","adoption_id":"a","day":"20260823","record_revision":1,"first_transition_serial":1,"dirty_by_transition_serial":1,"dirty_generation":1,"completed_generation":0,"auxiliary_time":"t","extra":1}"#,
            DurableRole::Record,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::UnknownField { field }) if field == "extra"
        ));
    }

    #[test]
    fn missing_serial_is_refused() {
        let error = parse_json::<DayRecord>(
            br#"{"schema_version":1,"journal_id":"j","root_id":"r","adoption_id":"a","day":"20260823","record_revision":1,"dirty_generation":1,"completed_generation":0,"auxiliary_time":"t"}"#,
            DurableRole::Record,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::MissingSerial)
        ));
    }

    #[test]
    fn wrong_serial_on_record_or_witness_is_refused_or_unknown() {
        let mut record = sample_record(1, 1, 0);
        record.dirty_by_transition_serial = 0;
        assert!(matches!(
            validate_record_numbers(&record).unwrap_err(),
            ConvergenceError::Refused(Refusal::PersistedZeroSerial)
        ));
    }
}
