// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditOutcome {
    pub kind: String,
    pub per_day_failures: BTreeMap<String, String>,
    pub global_record_written: bool,
    pub global_record_error: Option<String>,
    pub partial_error: bool,
}
/// Best effort by design: plain append, no fsync or atomic publication.
pub fn write_prune_audit(
    journal: &Path,
    kind: &str,
    record: &Value,
    per_day: &BTreeMap<String, String>,
    local_day: &str,
) -> AuditOutcome {
    let mut outcome = AuditOutcome {
        kind: kind.into(),
        ..AuditOutcome::default()
    };
    for (day, message) in per_day {
        let path = journal
            .join("chronicle")
            .join(day)
            .join("health")
            .join("retention.log");
        if let Some(parent) = path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            outcome
                .per_day_failures
                .insert(day.clone(), error.to_string());
            continue;
        }
        if OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| writeln!(file, "{message}"))
            .is_err()
        {
            outcome
                .per_day_failures
                .insert(day.clone(), "day log write failed".into());
        }
    }
    let global = journal
        .join("health/pruning-runs")
        .join(format!("{local_day}.jsonl"));
    match global
        .parent()
        .and_then(|parent| fs::create_dir_all(parent).ok())
        .ok_or_else(|| "audit directory creation failed".to_owned())
        .and_then(|_| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&global)
                .map_err(|error| error.to_string())
        })
        .and_then(|mut file| writeln!(file, "{}", record).map_err(|error| error.to_string()))
    {
        Ok(()) => outcome.global_record_written = true,
        Err(error) => outcome.global_record_error = Some(error),
    }
    outcome.partial_error =
        !outcome.per_day_failures.is_empty() || outcome.global_record_error.is_some();
    outcome
}
