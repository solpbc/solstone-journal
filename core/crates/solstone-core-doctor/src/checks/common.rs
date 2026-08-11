// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::context::CheckContext;
use solstone_core_journal_config::read_journal_config;
use solstone_core_observer::store::record::ObserverRecord;
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
    solstone_core_observer::store::reload::load_observers(&context.journal_path)
        .map_err(|error| error.to_string())
}
pub fn enabled(records: Vec<ObserverRecord>) -> Vec<ObserverRecord> {
    records
        .into_iter()
        .filter(|record| !record.revoked() && record.enabled() != Some(false))
        .collect()
}
