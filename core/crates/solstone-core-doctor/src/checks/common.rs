// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::context::CheckContext;
use solstone_core_journal_config::read_journal_config;
use solstone_core_observer::store::record::ObserverRecord;
use solstone_core_observer::store::reload::{load_observers, load_observers_with_inventory};
use solstone_core_observer::{
    AssessedObserverFact, DeliveryInspection, ObserverDeliveryFacts, Reach, inspect_loaded,
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
pub fn observers(context: &CheckContext) -> Result<Vec<ObserverRecord>, String> {
    load_observers(&context.journal_path).map_err(|error| error.to_string())
}

pub(crate) fn inspect_context(context: &CheckContext) -> DeliveryInspection {
    inspect_loaded(
        load_observers_with_inventory(&context.journal_path),
        context.now.timestamp_millis(),
    )
}

pub(crate) fn delivery_facts(inspection: &DeliveryInspection) -> ObserverDeliveryFacts {
    ObserverDeliveryFacts {
        registry: inspection.registry,
        assessed: inspection
            .assessed
            .iter()
            .map(|row| AssessedObserverFact {
                name: row.name.clone(),
                state: row.state,
                reach: row.reach,
            })
            .collect(),
        unassessed: inspection.unassessed.clone(),
    }
}
pub fn enabled(records: Vec<ObserverRecord>) -> Vec<ObserverRecord> {
    records
        .into_iter()
        .filter(|record| !record.revoked() && record.enabled() != Some(false))
        .collect()
}

pub(crate) fn delivery_reach_clause(reach: Reach) -> &'static str {
    match reach {
        Reach::Active | Reach::Stale => "it is still running, but it isn't adding to your journal",
        Reach::Offline => "the device appears offline and may be asleep",
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
