// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::context::CheckContext;
use crate::vocabulary::{
    AssessedClientFact, ClientDeliveryFacts, ClientRegistryState, UnassessedClientFact,
};
use solstone_core_journal_config::read_journal_config;
use solstone_core_sol_link::client_status::{
    ClientActivityState, ClientAssessment, ClientCaptureState, ClientInspection, ClientReach,
    ConnectionFreshness, inspect_clients_at,
};
pub fn config_backend(context: &CheckContext) -> Result<Option<String>, String> {
    let read = read_journal_config(&context.journal_path).map_err(|error| error.to_string())?;
    Ok(read.config.and_then(|config| {
        config
            .get("transcribe")
            .and_then(serde_json::Value::as_object)
            .and_then(|transcribe| {
                transcribe
                    .get("backend")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
    }))
}
pub fn clients(context: &CheckContext) -> Result<Vec<ClientAssessment>, String> {
    match inspect_context(context) {
        ClientInspection::Empty { clients, .. } | ClientInspection::Ready { clients, .. } => {
            Ok(clients)
        }
        ClientInspection::LedgerUnavailable { reason, .. } => {
            Err(format!("authorized client ledger unavailable: {reason:?}"))
        }
    }
}

pub(crate) fn inspect_context(context: &CheckContext) -> ClientInspection {
    inspect_clients_at(&context.journal_path, context.now.timestamp_millis())
}

pub(crate) fn delivery_facts(inspection: &ClientInspection) -> ClientDeliveryFacts {
    let registry = match inspection {
        ClientInspection::LedgerUnavailable { .. } => ClientRegistryState::RegistryUnknown,
        ClientInspection::Empty { .. } => ClientRegistryState::RegistryEmpty,
        ClientInspection::Ready { .. } => ClientRegistryState::RegistryComplete,
    };
    let Some(rows) = inspection_rows(inspection) else {
        return ClientDeliveryFacts {
            registry,
            assessed: Vec::new(),
            unassessed: Vec::new(),
        };
    };
    ClientDeliveryFacts {
        registry,
        assessed: rows
            .iter()
            .filter(|row| is_assessed_capture(row))
            .map(|row| AssessedClientFact {
                name: client_name(row),
                state: capture_state_name(row.capture_state).to_owned(),
                reach: reach_name(row).to_owned(),
            })
            .collect(),
        unassessed: rows
            .iter()
            .filter(|row| !is_assessed_capture(row))
            .map(|row| UnassessedClientFact {
                name: client_name(row),
                reason: match row.capture_state {
                    ClientCaptureState::NoCapture => "awaiting_first_delivery",
                    ClientCaptureState::Unknown => "activity_unavailable",
                    ClientCaptureState::Degraded
                    | ClientCaptureState::Active
                    | ClientCaptureState::Stale
                    | ClientCaptureState::Offline => unreachable!("assessed capture state"),
                }
                .to_owned(),
                reach: reach_name(row).to_owned(),
            })
            .collect(),
    }
}

/// All entries in the authorization projection are currently paired clients.
pub fn enabled(records: Vec<ClientAssessment>) -> Vec<ClientAssessment> {
    records
}

pub(crate) fn inspection_rows(inspection: &ClientInspection) -> Option<&[ClientAssessment]> {
    match inspection {
        ClientInspection::Empty { clients, .. } | ClientInspection::Ready { clients, .. } => {
            Some(clients)
        }
        ClientInspection::LedgerUnavailable { .. } => None,
    }
}

pub(crate) fn activity_unavailable(inspection: &ClientInspection) -> bool {
    matches!(
        inspection,
        ClientInspection::Empty {
            activity: ClientActivityState::Unreadable | ClientActivityState::Malformed,
            ..
        } | ClientInspection::Ready {
            activity: ClientActivityState::Unreadable | ClientActivityState::Malformed,
            ..
        } | ClientInspection::LedgerUnavailable {
            activity: ClientActivityState::Unreadable | ClientActivityState::Malformed,
            ..
        }
    )
}

pub(crate) fn is_ledger_unavailable(inspection: &ClientInspection) -> bool {
    matches!(inspection, ClientInspection::LedgerUnavailable { .. })
}

pub(crate) fn assessed_capture_rows(
    inspection: &ClientInspection,
) -> Option<Vec<&ClientAssessment>> {
    inspection_rows(inspection)
        .map(|rows| rows.iter().filter(|row| is_assessed_capture(row)).collect())
}

pub(crate) fn is_assessed_capture(row: &ClientAssessment) -> bool {
    matches!(
        row.capture_state,
        ClientCaptureState::Degraded
            | ClientCaptureState::Active
            | ClientCaptureState::Stale
            | ClientCaptureState::Offline
    )
}

pub(crate) fn client_name(row: &ClientAssessment) -> String {
    let label = row.client_entry.display_label();
    if label.is_empty() {
        row.cid.clone()
    } else {
        label
    }
}

pub(crate) fn capture_state_name(state: ClientCaptureState) -> &'static str {
    match state {
        ClientCaptureState::Unknown => "unknown",
        ClientCaptureState::NoCapture => "no_capture",
        ClientCaptureState::Degraded => "degraded",
        ClientCaptureState::Active => "active",
        ClientCaptureState::Stale => "stale",
        ClientCaptureState::Offline => "offline",
    }
}

pub(crate) fn reach_name(row: &ClientAssessment) -> &'static str {
    match row.connection {
        ConnectionFreshness::Unknown => "unknown",
        ConnectionFreshness::Known { reach, .. } => match reach {
            ClientReach::Active => "active",
            ClientReach::Stale => "stale",
            ClientReach::Offline => "offline",
        },
    }
}

pub(crate) fn delivery_reach_clause(reach: ClientReach) -> &'static str {
    match reach {
        ClientReach::Active | ClientReach::Stale => {
            "it is still running, but it isn't adding to your journal"
        }
        ClientReach::Offline => "the device appears offline and may be asleep",
    }
}

pub(crate) fn join_capped(clauses: &[String], separator: &str) -> String {
    let named = clauses.iter().take(3).cloned().collect::<Vec<_>>();
    let extra = clauses.len().saturating_sub(3);
    if extra == 0 {
        named.join(separator)
    } else {
        format!("{}{separator}+{extra} more", named.join(separator))
    }
}
