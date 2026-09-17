// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fail-closed audit publication and best-effort observation notification.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use solstone_core_callosum::CallosumOneShotSender;
use solstone_core_mcp_audit::{
    Admission, AuditCoordinates, AuditWriteError, Outcome, ResultShape, write_interaction_record,
    write_outcome_record,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

/// Durably publish one admitted interaction, then notify Callosum without affecting durability.
pub(crate) fn write_admitted_interaction(
    journal_root: &Path,
    now: DateTime<Utc>,
    admission: &Admission<'_>,
) -> Result<AuditCoordinates, AuditWriteError> {
    let coordinates = write_interaction_record(journal_root, now, admission)?;
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
    now: DateTime<Utc>,
    outcome: Outcome,
    reason: Option<&str>,
    result: Option<ResultShape>,
) -> Result<(), AuditWriteError> {
    write_outcome_record(journal_root, coordinates, now, outcome, reason, result)
}

fn emit_observed(journal_root: &Path, coordinates: &AuditCoordinates) {
    let Ok(line) = serde_json::to_string(&json!({
        "tract": "observe",
        "event": "observed",
        "day": coordinates.day.format("%Y%m%d").to_string(),
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
    use std::fs;
    use std::io::Read as _;
    use std::os::unix::net::UnixListener;
    use std::thread;

    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use solstone_core_mcp_audit::{Admission, ToolName};

    use super::write_admitted_interaction;

    #[test]
    fn observed_notification_contains_only_audit_coordinates() {
        let journal = tempfile::Builder::new()
            .prefix("solstone-mcp-audit-event-")
            .tempdir_in("/var/tmp")
            .expect("fixture journal");
        let health = journal.path().join("health");
        fs::create_dir_all(&health).expect("fixture health directory");
        let socket = health.join("callosum.sock");
        let listener = UnixListener::bind(&socket).expect("fixture Callosum listener");
        let received = thread::spawn(move || {
            let (mut connection, _) = listener.accept().expect("event sender connects");
            let mut line = String::new();
            connection
                .read_to_string(&mut line)
                .expect("event line reads");
            line
        });
        let now = Utc.with_ymd_and_hms(2026, 8, 31, 12, 34, 56).unwrap();

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
            serde_json::from_str::<serde_json::Value>(&received.join().expect("listener joins"))
                .expect("event is JSON"),
            json!({
                "tract": "observe",
                "event": "observed",
                "day": "20260831",
                "stream": "mcp.agent",
                "segment": "123456_1",
            })
        );
    }
}
