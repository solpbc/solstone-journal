// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fail-closed audit publication and best-effort observation notification.

mod reason;
pub(crate) use reason::Reason;

use std::path::Path;
use std::time::Duration;

use jiff::Timestamp;
use serde_json::json;
use solstone_core_callosum::CallosumOneShotSender;
use solstone_core_mcp_audit::{
    Admission, AuditCoordinates, AuditWriteError, Outcome, ResultShape, write_interaction_record,
    write_outcome_record,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

/// Durably publish one admitted interaction, then notify Callosum without affecting durability.
///
/// The record files under the owner-zone day and wall time, like every other
/// journal day directory, and keeps the UTC instant.
pub(crate) fn write_admitted_interaction(
    journal_root: &Path,
    now: Timestamp,
    admission: &Admission<'_>,
) -> Result<AuditCoordinates, AuditWriteError> {
    let zone = solstone_core_journal_config::owner_zone(journal_root);
    let coordinates =
        write_interaction_record(journal_root, &now.to_zoned_datetime(zone), admission)?;
    emit_observed(journal_root, &coordinates);
    Ok(coordinates)
}

/// Publish the outcome sibling of an admission already on disk.
///
/// ⚠ This is the release gate for a read: a prepared response whose outcome
/// cannot be recorded is refused rather than served, because an owner asking
/// "what did this connection see?" must not be answered by a record that says
/// only that something was asked.
pub(crate) fn write_outcome(
    journal_root: &Path,
    coordinates: &AuditCoordinates,
    now: Timestamp,
    outcome: Outcome,
    reason: Option<&str>,
    result: Option<ResultShape>,
) -> Result<(), AuditWriteError> {
    write_outcome_record(journal_root, coordinates, now, outcome, reason, result)
}

pub(crate) fn write_described_outcome(
    journal: &Path,
    coordinates: &AuditCoordinates,
    now: Timestamp,
    outcome: Outcome,
    reason: Reason,
    paused: bool,
) -> Result<(), AuditWriteError> {
    let english = reason.render(solstone_core_convey_http::window_catalog::ENGLISH);
    solstone_core_mcp_audit::write_outcome_with_description(
        journal,
        coordinates,
        now,
        solstone_core_mcp_audit::OutcomeDescription {
            outcome,
            reason: Some(&english),
            reason_copy: Some(serde_json::json!(reason)),
            result: None,
            paused,
        },
    )
}

fn emit_observed(journal_root: &Path, coordinates: &AuditCoordinates) {
    let Ok(line) = serde_json::to_string(&json!({
        "tract": "observe",
        "event": "observed",
        "day": solstone_core_time::day_key(coordinates.day),
        "stream": coordinates.stream,
        "segment": coordinates.segment,
    })) else {
        return;
    };
    let line = format!("{line}\n");
    let _ = CallosumOneShotSender::new(journal_root.join("health/callosum.sock"), SOCKET_TIMEOUT)
        .send_line(&line);
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use solstone_core_callosum::test_fixture::OneShotListener;

    use serde_json::json;
    use solstone_core_mcp_audit::{Admission, ToolName};

    use super::write_admitted_interaction;

    #[test]
    fn observed_notification_contains_only_audit_coordinates() {
        let journal = tempfile::Builder::new()
            .prefix("solstone-mcp-audit-event-")
            .tempdir_in(crate::test_scratch())
            .expect("fixture journal");
        let listener = OneShotListener::bind(journal.path().join("health/callosum.sock"));
        std::fs::create_dir_all(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            json!({"identity": {"timezone": "Pacific/Honolulu"}}).to_string(),
        )
        .unwrap();
        let now: jiff::Timestamp = "2026-08-31T00:34:56Z".parse().unwrap();
        let zone = solstone_core_journal_config::owner_zone(journal.path());
        let owner_dt = now.to_zoned_datetime(zone);
        assert_eq!(owner_dt.strftime("%Y%m%d").to_string(), "20260830");

        write_admitted_interaction(
            journal.path(),
            now,
            &Admission {
                connection: "operator",
                agent_identity: "operator",
                tool_name: ToolName::Search,
                arguments: serde_json::Map::new(),
                permission: None,
            },
        )
        .expect("audit record writes");

        assert_eq!(
            listener.finish(1).remove(0),
            json!({
                "tract": "observe",
                "event": "observed",
                "day": owner_dt.strftime("%Y%m%d").to_string(),
                "stream": "mcp.agent",
                "segment": owner_dt.strftime("%H%M%S_1").to_string(),
            })
        );
    }
}
