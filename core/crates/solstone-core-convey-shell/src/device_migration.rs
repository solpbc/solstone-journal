// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private takeover sequencer joining the link, segment, and push owners.

use std::path::Path;

use serde_json::Value;
use solstone_core_segment::{TakeoverPlan, with_takeover_stream_boundary};
use solstone_core_sol_link::device_migration::{
    DecisionApply, DecisionRequest, DecisionResponse, MigrationError, MigrationReasonCode, decide,
    pending_push_cid, record_push_result,
};

pub(crate) fn decide_and_resume(
    journal: &Path,
    caller_cid: &str,
    raw_body: &[u8],
) -> Result<DecisionResponse, MigrationError> {
    let mut boundary = |adopted: &str,
                        retired: &str,
                        stored_plan: Option<&Value>,
                        apply: &mut DecisionApply<'_>| {
        with_takeover_stream_boundary(journal, adopted, retired, |guard| {
            let plan = match stored_plan {
                Some(value) => serde_json::from_value::<TakeoverPlan>(value.clone())
                    .map_err(|_| segment_error())?,
                None => guard.plan(adopted, retired),
            };
            let plan_value = serde_json::to_value(&plan).map_err(|_| segment_error())?;
            let mut publish = |value: &Value| {
                let plan = serde_json::from_value::<TakeoverPlan>(value.clone())
                    .map_err(|_| unavailable())?;
                guard.publish(&plan).map_err(|_| unavailable())
            };
            Ok(apply(plan_value, &mut publish))
        })
        .map_err(|_| unavailable())?
    };

    let view = decide(journal, caller_cid, raw_body, &mut boundary)?;
    let request: DecisionRequest = serde_json::from_slice(raw_body).map_err(|_| unavailable())?;
    let Some(retired_cid) = pending_push_cid(journal, caller_cid, &request.operation_id)? else {
        return Ok(view);
    };
    if solstone_core_push::remove_cid_registrations(journal, &retired_cid).is_err() {
        return record_push_result(journal, caller_cid, &request.operation_id, false);
    }
    record_push_result(journal, caller_cid, &request.operation_id, true)
}

fn unavailable() -> MigrationError {
    MigrationError {
        reason: MigrationReasonCode::MigrationStateUnavailable,
    }
}

fn segment_error() -> solstone_core_segment::SegmentError {
    solstone_core_segment::SegmentError::StreamInput("invalid stored takeover plan")
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use solstone_core_sol_link::device_migration::{Choice, DecisionRequest};
    use solstone_core_sol_link::ledger::{AuthorizationLedger, ClientEntry, ClientRole};

    use super::decide_and_resume;

    const OLD: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ADOPTED: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const OPERATION: &str = "123e4567-e89b-42d3-a456-426614174000";
    const DECISION: &str = "123e4567-e89b-42d3-a456-426614174010";

    fn prepare(journal: &std::path::Path) -> Vec<u8> {
        let mut ledger = AuthorizationLedger::new(journal);
        ledger
            .add(ClientEntry::new(
                OLD,
                "Old phone",
                "2026-09-01T00:00:00Z",
                "fixture-instance",
                ClientRole::Roleless,
            ))
            .unwrap();
        ledger
            .add(ClientEntry::new(
                ADOPTED,
                "New phone",
                "2026-09-01T00:00:00Z",
                "fixture-instance",
                ClientRole::Roleless,
            ))
            .unwrap();
        let operation_dir = journal.join("link/device-migrations/operations");
        fs::create_dir_all(&operation_dir).unwrap();
        fs::write(
            operation_dir.join(format!("{OPERATION}.json")),
            serde_json::to_vec(&json!({
                "caller_cid": OLD,
                "operation_id": OPERATION,
                "raw_body": [],
                "issued_pem": "stored-certificate",
                "issued_cid": ADOPTED,
                "previous_cid": OLD,
                "pairing": {"client_cert": "stored-certificate"},
                "checkpoint": "issued",
                "generation": 1,
                "order": 1
            }))
            .unwrap(),
        )
        .unwrap();
        serde_json::to_vec(&DecisionRequest {
            protocol_version: 1,
            operation_id: DECISION.to_owned(),
            choice: Choice::SameDevice,
            replaces_cid: solstone_core_sol_link::device_migration::ReplacesCid::Missing,
        })
        .unwrap()
    }

    #[test]
    fn migration_push_removal_failure_stays_awaiting_push() {
        let journal = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let body = prepare(journal.path());
        fs::create_dir_all(journal.path().join("config")).unwrap();
        let push_path = journal.path().join("config/push-registry.json");
        fs::write(&push_path, b"{ malformed").unwrap();
        let before = fs::read(&push_path).unwrap();

        let error = decide_and_resume(journal.path(), ADOPTED, &body).unwrap_err();

        assert_eq!(error.reason_code(), "migration_state_unavailable");
        assert_eq!(fs::read(&push_path).unwrap(), before);
        let decision_path = journal
            .path()
            .join(format!("link/device-migrations/decisions/{DECISION}.json"));
        let decision: Value = serde_json::from_slice(&fs::read(&decision_path).unwrap()).unwrap();
        assert_eq!(decision["checkpoint"], "awaiting_push");
        assert_eq!(decision["replaced_cid"], OLD);

        fs::write(&push_path, br#"{"version":2,"devices":[]}"#).unwrap();
        let resumed = decide_and_resume(journal.path(), ADOPTED, &body).unwrap();
        assert_eq!(resumed.state.as_wire(), "same_device");
        let decision: Value = serde_json::from_slice(&fs::read(&decision_path).unwrap()).unwrap();
        assert_eq!(decision["checkpoint"], "complete");
    }
}
