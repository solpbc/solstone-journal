// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable unfinished activity work. Think owns writes; the supervisor reads
//! due identities and submits the existing activity command. Completed work is
//! represented by the existing input provenance, not a second success ledger.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, LockOptions, atomic_replace, hold_lock,
};

use crate::context::ThinkContext;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityRetry {
    pub day: String,
    pub facet: String,
    pub activity: String,
}

#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    identity: ActivityRetry,
    input_hash: String,
    remaining: BTreeSet<String>,
    uses: BTreeMap<String, String>,
    #[serde(default)]
    parked: BTreeSet<String>,
    attempts: u32,
    next_attempt_ms: i64,
}

pub(crate) struct ActivityWork {
    path: PathBuf,
    record: Record,
    _claim: FileLock,
}

fn directory(journal: &Path) -> PathBuf {
    journal.join("health/activity-work")
}

fn path(journal: &Path, identity: &ActivityRetry) -> Result<PathBuf, String> {
    chrono::NaiveDate::parse_from_str(&identity.day, "%Y%m%d").map_err(|e| e.to_string())?;
    for value in [&identity.facet, &identity.activity] {
        if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\\', '\0']) {
            return Err("invalid activity work identity".to_owned());
        }
    }
    let bytes = serde_json::to_vec(identity).map_err(|e| e.to_string())?;
    Ok(directory(journal).join(format!("{:x}.json", Sha256::digest(bytes))))
}

fn read(path: &Path) -> Result<Option<Record>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let record: Record = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if record.version != 1 {
                return Err("unsupported activity work version".to_owned());
            }
            Ok(Some(record))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// A connection failure never exhausts its retries. Only the delay is capped.
fn backoff_ms(attempts: u32) -> i64 {
    (60_000_i64 * (1_i64 << attempts.saturating_sub(1).min(6))).min(3_600_000)
}

impl ActivityWork {
    pub(crate) fn begin(
        context: &ThinkContext,
        facet: &str,
        activity: &str,
        input_hash: String,
        names: BTreeSet<String>,
        refresh: bool,
    ) -> Result<Self, String> {
        let identity = ActivityRetry {
            day: context.day.clone(),
            facet: facet.to_owned(),
            activity: activity.to_owned(),
        };
        let path = path(&context.journal, &identity)?;
        std::fs::create_dir_all(directory(&context.journal)).map_err(|e| e.to_string())?;
        // One per-activity claim covers read/dispatch/completion, so a manual
        // command cannot race a queued retry. Other activities remain independent.
        let claim_path =
            crate::segment::activity_provenance_path(context, &context.day, facet, activity);
        std::fs::create_dir_all(claim_path.parent().expect("provenance parent"))
            .map_err(|e| e.to_string())?;
        let claim = hold_lock(
            &claim_path,
            LockOptions {
                timeout: Duration::ZERO,
                ..LockOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let old = read(&path)?;
        if let Some(old) = old.as_ref()
            && (refresh || old.input_hash != input_hash)
        {
            for id in old.uses.values() {
                if solstone_core_cortex_client::use_file_status(
                    &context.journal.join("talents"),
                    id,
                )
                .map_err(|e| e.to_string())?
                    == solstone_core_cortex_client::UseFileStatus::Running
                {
                    return Err("previous activity input is still running".to_owned());
                }
            }
        }
        let mut names = names;
        if old.is_none() && !refresh {
            let provenance =
                crate::segment::activity_provenance_path(context, &context.day, facet, activity);
            if let Ok(bytes) = std::fs::read(provenance)
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
                && value["input_hash"].as_str() == Some(input_hash.as_str())
            {
                names.clear();
            }
        }
        let record = match old {
            Some(mut record) if !refresh && record.input_hash == input_hash => {
                if record.identity != identity {
                    return Err("activity work identity mismatch".to_owned());
                }
                // Disabled/removed talents stop being eligible; completed siblings
                // stay absent. A new input revision starts a fresh unit set.
                record.remaining.retain(|name| names.contains(name));
                record
            }
            _ => Record {
                version: 1,
                identity,
                input_hash,
                remaining: names,
                uses: BTreeMap::new(),
                parked: BTreeSet::new(),
                attempts: 0,
                next_attempt_ms: 0,
            },
        };
        let work = Self {
            path,
            record,
            _claim: claim,
        };
        work.save()?;
        Ok(work)
    }

    pub(crate) fn due(&self, now_ms: i64) -> bool {
        self.record.remaining.is_empty() || self.record.next_attempt_ms <= now_ms
    }
    pub(crate) fn parked(&self, name: &str) -> bool {
        self.record.parked.contains(name)
    }
    pub(crate) fn park(&mut self, name: &str) -> Result<(), String> {
        self.record.parked.insert(name.to_owned());
        self.save()
    }
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.record.remaining.contains(name)
    }
    pub(crate) fn use_id(&self, name: &str) -> Option<&str> {
        self.record.uses.get(name).map(String::as_str)
    }
    pub(crate) fn start_attempt(&mut self, now_ms: i64) -> Result<(), String> {
        self.record.attempts = self.record.attempts.saturating_add(1);
        self.record.next_attempt_ms = now_ms.saturating_add(backoff_ms(self.record.attempts));
        self.save()
    }
    pub(crate) fn dispatched(&mut self, name: &str, use_id: &str) -> Result<(), String> {
        self.record.uses.insert(name.to_owned(), use_id.to_owned());
        self.save()
    }
    pub(crate) fn complete(&mut self, name: &str) -> Result<(), String> {
        self.record.remaining.remove(name);
        self.record.uses.remove(name);
        self.save()
    }
    pub(crate) fn finish(&mut self, context: &ThinkContext) -> Result<(), String> {
        if self.record.remaining.is_empty() {
            crate::segment::write_activity_provenance(
                context,
                &self.record.identity.day,
                &self.record.identity.facet,
                &self.record.identity.activity,
                &self.record.input_hash,
            )?;
            // If interrupted after publication, a later retry sees the completed
            // empty record and repeats only this cleanup, never model execution.
            std::fs::remove_file(&self.path).map_err(|e| e.to_string())?;
        } else {
            self.record.next_attempt_ms = context
                .event_now_ms()
                .saturating_add(backoff_ms(self.record.attempts));
            self.save()?;
        }
        Ok(())
    }
    fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&self.record).map_err(|e| e.to_string())?;
        atomic_replace(&self.path, &bytes, AtomicWriteOptions::default()).map_err(|e| e.to_string())
    }
}

/// Read-only discovery has no date horizon: an interrupted older activity stays
/// eligible until it succeeds, even after restart or a prolonged outage.
pub fn due_activity_retries(journal: &Path, now_ms: i64) -> Result<Vec<ActivityRetry>, String> {
    let entries = match std::fs::read_dir(directory(journal)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut due = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(record) = read(&entry.path())? else {
            continue;
        };
        if path(journal, &record.identity)? != entry.path() {
            return Err("activity work filename mismatch".to_owned());
        }
        if record.next_attempt_ms <= now_ms
            && (record.remaining.is_empty()
                || record.remaining.iter().any(|n| !record.parked.contains(n)))
        {
            due.push(record.identity);
        }
    }
    due.sort_by(|a, b| (&a.day, &a.facet, &a.activity).cmp(&(&b.day, &b.facet, &b.activity)));
    Ok(due)
}

/// Adopt unfinished connection failures from a supplied recent source day on
/// upgrade. Existing work records are never replaced. Historical days are not
/// implicitly regenerated; durable records themselves have no age cutoff.
pub fn seed_activity_retries(journal: &Path, day: &str, now_ms: i64) -> Result<(), String> {
    use solstone_core_system_health::{
        FilesystemHealthLogSource, TerminalEvent, read_terminal_states,
    };
    let states = read_terminal_states(&FilesystemHealthLogSource::new(journal), day, true)
        .map_err(|e| e.to_string())?;
    if states.malformed_line_count != 0 {
        return Err("cannot seed activity retries from malformed health logs".to_owned());
    }
    let mut grouped = BTreeMap::<(String, String), BTreeMap<String, String>>::new();
    for (unit, state) in states.value {
        if unit.mode != "activity"
            || state.latest_event != TerminalEvent::Fail
            || !matches!(
                state.reason_code.as_deref(),
                Some("local_endpoint_unreachable" | "send_failed" | "request_lost")
            )
        {
            continue;
        }
        let (Some(facet), Some(activity)) = (unit.facet, unit.activity) else {
            continue;
        };
        grouped
            .entry((facet, activity))
            .or_default()
            .insert(unit.name, state.use_id.unwrap_or_default());
    }
    if grouped.is_empty() {
        return Ok(());
    }
    let context = ThinkContext::new_with_event_clock(
        journal,
        day.to_owned(),
        journal.join("chronicle").join(day),
        now_ms,
        std::sync::Arc::new(move || now_ms),
    )?;
    for ((facet, activity), uses) in grouped {
        let identity = ActivityRetry {
            day: day.to_owned(),
            facet: facet.clone(),
            activity: activity.clone(),
        };
        let path = path(journal, &identity)?;
        if path.try_exists().map_err(|e| e.to_string())? {
            continue;
        }
        std::fs::create_dir_all(directory(journal)).map_err(|e| e.to_string())?;
        let claim_path = crate::segment::activity_provenance_path(&context, day, &facet, &activity);
        std::fs::create_dir_all(claim_path.parent().expect("provenance parent"))
            .map_err(|e| e.to_string())?;
        let claim = hold_lock(
            &claim_path,
            LockOptions {
                timeout: Duration::from_secs(1),
                ..LockOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
        if read(&path)?.is_some() {
            continue;
        }
        let Some(record) =
            solstone_core_facets::get_activity_record(journal, &facet, day, &activity)
                .map_err(|e| e.to_string())?
        else {
            continue;
        };
        let input_hash = crate::segment::compute_activity_input_hash(&context, day, &record)
            .ok_or_else(|| "cannot fingerprint interrupted activity".to_owned())?;
        ActivityWork {
            path,
            _claim: claim,
            record: Record {
                version: 1,
                identity,
                input_hash,
                remaining: uses.keys().cloned().collect(),
                uses: uses.into_iter().filter(|(_, id)| !id.is_empty()).collect(),
                parked: BTreeSet::new(),
                attempts: 0,
                next_attempt_ms: 0,
            },
        }
        .save()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_retries_back_off_without_exhaustion() {
        assert_eq!(backoff_ms(1), 60_000);
        assert_eq!(backoff_ms(2), 120_000);
        assert_eq!(backoff_ms(20), 3_600_000);
        assert_eq!(backoff_ms(u32::MAX), 3_600_000);
    }
}
