// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable retry permission, never output acceptance. Callers must hold the
//! owning unit's execution claim while consuming permission and doing work.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};

pub mod daily;
pub mod input;
pub mod raw;
pub use raw::RetryContext;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Unit {
    Raw {
        day: String,
        stream: String,
        segment: String,
        handler: String,
        output: String,
        direct: bool,
    },
    Segment {
        day: String,
        stream: String,
        segment: String,
        talent: String,
        direct: bool,
    },
    Daily {
        day: String,
        talent: String,
        facet: Option<String>,
    },
    Activity {
        day: String,
        facet: String,
        activity: String,
        talent: String,
    },
}

impl Unit {
    pub fn day(&self) -> &str {
        match self {
            Self::Raw { day, .. }
            | Self::Segment { day, .. }
            | Self::Daily { day, .. }
            | Self::Activity { day, .. } => day,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.day().len() != 8 || chrono::NaiveDate::parse_from_str(self.day(), "%Y%m%d").is_err()
        {
            return Err("invalid retry day".into());
        }
        let component = |value: &str| {
            !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\', '\0'])
        };
        let valid = match self {
            Self::Raw {
                stream,
                segment,
                handler,
                output,
                ..
            } => {
                component(stream)
                    && component(segment)
                    && component(output)
                    && output.ends_with(".jsonl")
                    && matches!(handler.as_str(), "describe" | "depict" | "transcribe")
            }
            Self::Segment {
                stream,
                segment,
                talent,
                ..
            } => component(stream) && component(segment) && component(talent),
            Self::Daily { talent, facet, .. } => {
                component(talent) && facet.as_deref().is_none_or(component)
            }
            Self::Activity {
                facet,
                activity,
                talent,
                ..
            } => component(facet) && component(activity) && component(talent),
        };
        if valid {
            Ok(())
        } else {
            Err("invalid retry unit".into())
        }
    }

    pub fn id(&self) -> Result<String, String> {
        self.validate()?;
        let mut identity = self.clone();
        if let Self::Daily { day, talent, facet } = &mut identity
            && talent == "daily_schedule"
        {
            day.clear();
            *facet = None;
        }
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&identity).map_err(|e| e.to_string())?)
        ))
    }

    pub fn segment_directory(&self, journal: &Path) -> Result<PathBuf, String> {
        self.validate()?;
        match self {
            Self::Raw {
                day,
                stream,
                segment,
                direct,
                ..
            }
            | Self::Segment {
                day,
                stream,
                segment,
                direct,
                ..
            } => {
                let day = journal.join("chronicle").join(day);
                Ok(if *direct {
                    day.join(segment)
                } else {
                    day.join(stream).join(segment)
                })
            }
            _ => Err("retry unit is not a recording".into()),
        }
    }
}

pub fn file_fingerprint(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    if !before.is_file() {
        return Err("retry input is not a regular file".into());
    }
    let mut hash = Sha256::new();
    let mut bytes = [0_u8; 65536];
    loop {
        let count = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    let after = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if before.len() != after.len()
        || before.modified().map_err(|e| e.to_string())?
            != after.modified().map_err(|e| e.to_string())?
    {
        return Err("retry input changed while reading".into());
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub cause: String,
    pub input: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    version: u32,
    pub unit: Unit,
    pub held: bool,
    pub reason: String,
    pub input: String,
    pub owner: Option<String>,
    pub consumed: BTreeSet<String>,
    pub attempt: Option<Attempt>,
    #[serde(default)]
    failure_count: u32,
    #[serde(default)]
    first_failure_ms: Option<i64>,
}

/// Entries are explicit shipped fixes, not an application-version reset.
pub struct DeclaredFix {
    pub id: &'static str,
    pub reason: &'static str,
    pub introduced: &'static str,
}

pub const DECLARED_FIXES: &[DeclaredFix] = &[];

fn fix_available(fix: &DeclaredFix) -> bool {
    let version = |text: &str| -> Option<[u64; 3]> {
        let values = text
            .split('.')
            .map(str::parse)
            .collect::<Result<Vec<u64>, _>>()
            .ok()?;
        values.try_into().ok()
    };
    version(fix.introduced)
        .zip(version(env!("CARGO_PKG_VERSION")))
        .is_some_and(|(introduced, running)| introduced <= running)
}

fn data_verdict(reason: &str) -> bool {
    matches!(reason, "corrupt_input" | "no_audio_stream")
}

fn input_cause(input: &str) -> String {
    format!("input:{input}")
}

impl Record {
    /// Read-only eligibility. The owning reader supplies current relevant
    /// inputs; an unreadable input must never be represented as a new digest.
    pub fn permission(&self, input: &str, fixes: &[DeclaredFix]) -> Option<Attempt> {
        if !self.held || input.is_empty() {
            return None;
        }
        let changed = input != self.input;
        if data_verdict(&self.reason) && !changed {
            return None;
        }
        let mut causes = Vec::new();
        if let Some(owner) = &self.owner {
            causes.push(format!("owner:{owner}"));
        }
        if changed {
            causes.push(input_cause(input));
        }
        if !data_verdict(&self.reason) {
            causes.extend(
                fixes
                    .iter()
                    .filter(|fix| fix.reason == self.reason && fix_available(fix))
                    .map(|fix| format!("fix:{}", fix.id)),
            );
        }
        causes
            .into_iter()
            .find(|cause| !self.consumed.contains(cause))
            .map(|cause| Attempt {
                cause,
                input: input.into(),
                reason: self.reason.clone(),
            })
    }
}

fn directory(journal: &Path) -> PathBuf {
    journal.join("health/work-retry")
}

fn path(journal: &Path, id: &str) -> Result<PathBuf, String> {
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid retry identity".into());
    }
    Ok(directory(journal).join(format!("{id}.json")))
}

pub fn read(journal: &Path, id: &str) -> Result<Option<Record>, String> {
    let bytes = match std::fs::read(path(journal, id)?) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let record: Record = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if record.version != 1
        || record.unit.id()? != id
        || record.input.is_empty()
        || record.reason.is_empty()
        || record
            .owner
            .as_ref()
            .is_some_and(|id| id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
        || !record.consumed.contains(&input_cause(&record.input))
        || record.attempt.as_ref().is_some_and(|attempt| {
            !record.consumed.contains(&attempt.cause) || attempt.input != record.input
        })
    {
        return Err("unrecognized retry record".into());
    }
    Ok(Some(record))
}

fn update<T>(
    journal: &Path,
    unit: &Unit,
    mutate: impl FnOnce(&mut Option<Record>) -> Result<T, String>,
) -> Result<T, String> {
    update_checked(journal, unit, &|| Ok(()), mutate)
}

fn update_checked<T>(
    journal: &Path,
    unit: &Unit,
    checkpoint: &dyn Fn() -> Result<(), String>,
    mutate: impl FnOnce(&mut Option<Record>) -> Result<T, String>,
) -> Result<T, String> {
    let id = unit.id()?;
    let path = path(journal, &id)?;
    std::fs::create_dir_all(directory(journal)).map_err(|e| e.to_string())?;
    let _lock = hold_lock(&path, LockOptions::default()).map_err(|e| e.to_string())?;
    checkpoint()?;
    let mut record = read(journal, &id)?;
    let result = mutate(&mut record)?;
    if let Some(record) = record {
        checkpoint()?;
        write_json(path, &record, JsonWriteOptions::default()).map_err(|e| e.to_string())?;
    }
    Ok(result)
}

/// Seed legacy evidence conservatively. Repeated discovery cannot reset an
/// attempt or adopt changed input as its baseline. Owning writers use finish.
pub fn observe_held(journal: &Path, unit: &Unit, reason: &str, input: &str) -> Result<(), String> {
    observe_held_checked(journal, unit, reason, input, &|| Ok(()))
}

pub fn observe_held_checked(
    journal: &Path,
    unit: &Unit,
    reason: &str,
    input: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    if reason.is_empty() || input.is_empty() {
        return Err("missing held evidence".into());
    }
    update_checked(journal, unit, checkpoint, |slot| {
        if let Some(record) = slot {
            record.reason = reason.into();
            if !record.held {
                record.held = true;
                record.input = input.into();
                record.consumed.insert(input_cause(input));
            }
        } else {
            *slot = Some(Record {
                version: 1,
                unit: unit.clone(),
                held: true,
                reason: reason.into(),
                input: input.into(),
                owner: None,
                consumed: BTreeSet::from([input_cause(input)]),
                attempt: None,
                failure_count: 0,
                first_failure_ms: None,
            });
        }
        Ok(())
    })
}

/// Independent failures remain observable if a sibling record is malformed.
pub fn list(journal: &Path) -> (Vec<Record>, Vec<String>) {
    let entries = match std::fs::read_dir(directory(journal)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (vec![], vec![]),
        Err(e) => return (vec![], vec![e.to_string()]),
    };
    let mut records = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                errors.push(e.to_string());
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        match read(journal, id) {
            Ok(Some(record))
                if matches!(&record.unit, Unit::Daily { talent, .. }
                    if crate::daily_coverage::RETIRED_DAILY_TALENTS.contains(&talent.as_str())) => {
            }
            Ok(Some(record)) if record.held => records.push(record),
            Ok(_) => {}
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    records.sort_by(|a, b| b.unit.day().cmp(a.unit.day()));
    (records, errors)
}

/// Generic talent execution exclusion. Daily and native writers use their
/// existing domain claims. Never hold the record lock while acquiring it.
pub fn execution_claim(
    journal: &Path,
    unit: &Unit,
) -> Result<solstone_core_journal_io::FileLock, String> {
    let path = path(journal, &unit.id()?)?.with_extension("execution");
    std::fs::create_dir_all(directory(journal)).map_err(|e| e.to_string())?;
    hold_lock(
        path,
        LockOptions {
            timeout: std::time::Duration::ZERO,
            ..LockOptions::default()
        },
    )
    .map_err(|e| e.to_string())
}

pub fn request(journal: &Path, unit: &Unit) -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    update(journal, unit, |slot| {
        let record = slot
            .as_mut()
            .filter(|r| r.held)
            .ok_or("unit is no longer held")?;
        record.owner = Some(id.clone());
        Ok(())
    })?;
    Ok(id)
}

/// Expand an existing durable day intent once for this unit. Repeated scans
/// must not turn the same owner ask into fresh permission after consumption.
pub fn request_with_id(journal: &Path, unit: &Unit, id: &str) -> Result<(), String> {
    request_with_id_checked(journal, unit, id, &|| Ok(()))
}

pub fn request_with_id_checked(
    journal: &Path,
    unit: &Unit,
    id: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid owner retry identity".into());
    }
    update_checked(journal, unit, checkpoint, |slot| {
        let record = slot
            .as_mut()
            .filter(|r| r.held)
            .ok_or("unit is no longer held")?;
        if record
            .owner
            .as_ref()
            .is_none_or(|owner| record.consumed.contains(&format!("owner:{owner}")))
            && !record.consumed.contains(&format!("owner:{id}"))
        {
            record.owner = Some(id.to_owned());
        }
        Ok(())
    })
}

/// Call only under the unit's execution claim, after re-reading its failure
/// and inputs. Persisted consumption precedes all model or publication work.
pub fn consume(
    journal: &Path,
    unit: &Unit,
    selected: &Attempt,
    reason: &str,
    input: &str,
    fixes: &[DeclaredFix],
) -> Result<(), String> {
    consume_checked(journal, unit, selected, reason, input, fixes, &|| Ok(()))
}

pub fn consume_checked(
    journal: &Path,
    unit: &Unit,
    selected: &Attempt,
    reason: &str,
    input: &str,
    fixes: &[DeclaredFix],
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    update_checked(journal, unit, checkpoint, |slot| {
        let record = slot.as_mut().ok_or("missing retry evidence")?;
        if reason != record.reason
            || selected.reason != reason
            || selected.input != input
            || record.permission(input, fixes).as_ref() != Some(selected)
        {
            return Err("retry permission changed".into());
        }
        record.consumed.insert(selected.cause.clone());
        // One attempt satisfies all currently available causes for these
        // inputs. Otherwise an owner ask plus changed input could run twice.
        record.consumed.insert(input_cause(input));
        for fix in fixes
            .iter()
            .filter(|fix| fix.reason == reason && fix_available(fix))
        {
            record.consumed.insert(format!("fix:{}", fix.id));
        }
        record.input = input.into();
        record.attempt = Some(selected.clone());
        Ok(())
    })
}

/// The caller has observed the actual writer's terminal outcome. A newer owner
/// request survives; terminal bookkeeping never certifies an output current.
pub fn finish(
    journal: &Path,
    unit: &Unit,
    attempt: &Attempt,
    remaining_reason: Option<&str>,
) -> Result<(), String> {
    finish_checked(journal, unit, attempt, remaining_reason, &|| Ok(()))
}

pub fn finish_checked(
    journal: &Path,
    unit: &Unit,
    attempt: &Attempt,
    remaining_reason: Option<&str>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    update_checked(journal, unit, checkpoint, |slot| {
        let record = slot.as_mut().ok_or("missing retry evidence")?;
        if record.attempt.as_ref() != Some(attempt) {
            return Ok(());
        }
        record.attempt = None;
        if record
            .owner
            .as_ref()
            .is_some_and(|id| attempt.cause == format!("owner:{id}"))
        {
            record.owner = None;
        }
        record.held = remaining_reason.is_some();
        if let Some(reason) = remaining_reason {
            record.reason = reason.into();
        }
        Ok(())
    })
}

/// The domain owner has reconciled an accepted terminal receipt while holding
/// its execution claim. Acceptance stays in that domain's durable record.
pub fn reconcile_accepted(journal: &Path, unit: &Unit) -> Result<(), String> {
    reconcile_accepted_checked(journal, unit, &|| Ok(()))
}

pub fn reconcile_accepted_checked(
    journal: &Path,
    unit: &Unit,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    update_checked(journal, unit, checkpoint, |slot| {
        if let Some(record) = slot {
            record.held = false;
            record.attempt = None;
            if record
                .owner
                .as_ref()
                .is_some_and(|owner| record.consumed.contains(&format!("owner:{owner}")))
            {
                record.owner = None;
            }
        }
        Ok(())
    })
}

/// Writer-owned floor disposition, outside prunable diagnostic logs. The
/// caller holds execution exclusion and supplies the input seen before work.
pub fn record_floor_outcome(
    journal: &Path,
    unit: &Unit,
    input: &str,
    failure: Option<&str>,
    now_ms: i64,
) -> Result<(), String> {
    record_floor_outcome_checked(journal, unit, input, failure, now_ms, &|| Ok(()))
}

pub fn record_floor_outcome_checked(
    journal: &Path,
    unit: &Unit,
    input: &str,
    failure: Option<&str>,
    now_ms: i64,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    update_checked(journal, unit, checkpoint, |slot| {
        let record = slot.get_or_insert_with(|| Record {
            version: 1,
            unit: unit.clone(),
            held: false,
            reason: "floor_failed".into(),
            input: input.into(),
            owner: None,
            consumed: BTreeSet::from([input_cause(input)]),
            attempt: None,
            failure_count: 0,
            first_failure_ms: None,
        });
        match failure {
            None => {
                record.held = false;
                record.failure_count = 0;
                record.first_failure_ms = None;
            }
            Some(reason) => {
                if record.input != input && !record.held {
                    record.failure_count = 0;
                    record.first_failure_ms = None;
                    record.input = input.into();
                    record.consumed.insert(input_cause(input));
                }
                record.reason = reason.into();
                record.failure_count = record.failure_count.saturating_add(1);
                let first = *record.first_failure_ms.get_or_insert(now_ms);
                record.held |=
                    record.failure_count >= 5 && now_ms.saturating_sub(first) >= 7_200_000;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "full-tests")]
    #[test]
    fn cancelled_retry_terminal_publication_after_record_lock_wait_and_positive_twins() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, mpsc};
        use std::time::Duration;
        for finishing in [false, true] {
            for cancel in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let unit = unit();
                let id = unit.id().unwrap();
                observe_held(root.path(), &unit, "schema_invalid", "input-a").unwrap();
                request(root.path(), &unit).unwrap();
                let grant = read(root.path(), &id)
                    .unwrap()
                    .unwrap()
                    .permission("input-a", &[])
                    .unwrap();
                consume(root.path(), &unit, &grant, "schema_invalid", "input-a", &[]).unwrap();
                let path = path(root.path(), &id).unwrap();
                let before = std::fs::read(&path).unwrap();
                let held = hold_lock(&path, LockOptions::default()).unwrap();
                let cancelled = Arc::new(AtomicBool::new(false));
                let flag = cancelled.clone();
                let journal = root.path().to_path_buf();
                let (started, ready) = mpsc::channel();
                let (checkpoint, reached) = mpsc::channel();
                let worker = std::thread::spawn(move || {
                    started.send(()).unwrap();
                    let check = || {
                        checkpoint.send(()).unwrap();
                        if flag.load(Ordering::SeqCst) {
                            Err("cancelled".into())
                        } else {
                            Ok(())
                        }
                    };
                    if finishing {
                        finish_checked(&journal, &unit, &grant, None, &check)
                    } else {
                        record_floor_outcome_checked(
                            &journal,
                            &unit,
                            "input-a",
                            Some("transport"),
                            1,
                            &check,
                        )
                    }
                });
                ready.recv_timeout(Duration::from_secs(2)).unwrap();
                assert!(reached.recv_timeout(Duration::from_millis(100)).is_err());
                cancelled.store(cancel, Ordering::SeqCst);
                drop(held);
                let result = worker.join().unwrap();
                if cancel {
                    assert_eq!(result.unwrap_err(), "cancelled");
                    assert_eq!(std::fs::read(&path).unwrap(), before);
                } else {
                    result.unwrap();
                    assert_ne!(std::fs::read(&path).unwrap(), before);
                }
            }
        }
    }

    use super::*;
    #[test]
    fn floor_hold_outlives_logs_and_has_no_version_or_tick_reset() {
        let root = tempfile::tempdir().unwrap();
        let unit = Unit::Segment {
            day: "20260101".into(),
            stream: "desktop".into(),
            segment: "120000_300".into(),
            talent: "documents".into(),
            direct: false,
        };
        for index in 0..5 {
            record_floor_outcome(
                root.path(),
                &unit,
                "input",
                Some("schema_invalid"),
                index * 1_800_000,
            )
            .unwrap();
        }
        let record = read(root.path(), &unit.id().unwrap()).unwrap().unwrap();
        assert!(record.held);
        assert!(record.permission("input", DECLARED_FIXES).is_none());
        assert!(record.permission("changed", DECLARED_FIXES).is_some());
        // There are no diagnostic logs in this journal at all.
        assert!(!root.path().join("chronicle").exists());
        assert_eq!(list(root.path()).0.len(), 1);
    }

    fn unit() -> Unit {
        Unit::Daily {
            day: "20260101".into(),
            talent: "summary".into(),
            facet: None,
        }
    }
    const FIXES: &[DeclaredFix] = &[DeclaredFix {
        id: "fixed-parser",
        reason: "schema_invalid",
        introduced: "2.0.0",
    }];

    #[test]
    fn held_retries_of_a_retired_daily_talent_are_not_listed() {
        let root = tempfile::tempdir().unwrap();
        let daily = |talent: &str| Unit::Daily {
            day: "20260910".into(),
            talent: talent.into(),
            facet: None,
        };
        observe_held(root.path(), &daily("schedule"), "schema_invalid", "input-a").unwrap();
        observe_held(
            root.path(),
            &daily("morning_briefing"),
            "schema_invalid",
            "input-a",
        )
        .unwrap();
        let (records, errors) = list(root.path());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].unit, daily("morning_briefing"));
    }

    #[test]
    fn causes_are_consumed_before_work_and_new_intent_survives_old_completion() {
        let root = tempfile::tempdir().unwrap();
        let unit = unit();
        let id = unit.id().unwrap();
        observe_held(root.path(), &unit, "schema_invalid", "input-a").unwrap();
        assert!(
            read(root.path(), &id)
                .unwrap()
                .unwrap()
                .permission("input-a", &[])
                .is_none()
        );
        request(root.path(), &unit).unwrap();
        let first = read(root.path(), &id)
            .unwrap()
            .unwrap()
            .permission("input-a", &[])
            .unwrap();
        consume(root.path(), &unit, &first, "schema_invalid", "input-a", &[]).unwrap();
        assert!(consume(root.path(), &unit, &first, "schema_invalid", "input-a", &[]).is_err());
        assert!(
            read(root.path(), &id)
                .unwrap()
                .unwrap()
                .permission("input-a", &[])
                .is_none()
        );
        let later = request(root.path(), &unit).unwrap();
        finish(root.path(), &unit, &first, Some("schema_invalid")).unwrap();
        assert_eq!(
            read(root.path(), &id).unwrap().unwrap().owner.as_deref(),
            Some(later.as_str())
        );
    }

    #[test]
    fn changed_input_and_declared_fix_do_not_recreate_each_others_budget() {
        let root = tempfile::tempdir().unwrap();
        let unit = unit();
        let id = unit.id().unwrap();
        observe_held(root.path(), &unit, "schema_invalid", "input-a").unwrap();
        let grant = read(root.path(), &id)
            .unwrap()
            .unwrap()
            .permission("input-b", FIXES)
            .unwrap();
        consume(
            root.path(),
            &unit,
            &grant,
            "schema_invalid",
            "input-b",
            FIXES,
        )
        .unwrap();
        finish(root.path(), &unit, &grant, Some("schema_invalid")).unwrap();
        observe_held(root.path(), &unit, "schema_invalid", "input-b").unwrap();
        let record = read(root.path(), &id).unwrap().unwrap();
        assert!(record.permission("input-b", FIXES).is_none());
        assert!(record.permission("input-a", FIXES).is_none());
    }

    #[test]
    fn unchanged_data_verdict_and_corrupt_permission_refuse() {
        let root = tempfile::tempdir().unwrap();
        let unit = unit();
        let id = unit.id().unwrap();
        observe_held(root.path(), &unit, "corrupt_input", "input-a").unwrap();
        request(root.path(), &unit).unwrap();
        let record = read(root.path(), &id).unwrap().unwrap();
        assert!(record.permission("input-a", FIXES).is_none());
        assert!(record.permission("input-b", FIXES).is_some());
        std::fs::write(path(root.path(), &id).unwrap(), "{").unwrap();
        assert!(observe_held(root.path(), &unit, "schema_invalid", "input-c").is_err());
        assert!(request(root.path(), &unit).is_err());
        assert_eq!(
            std::fs::read_to_string(path(root.path(), &id).unwrap()).unwrap(),
            "{"
        );
    }

    #[test]
    fn one_declared_fix_is_consumed_once_by_racing_workers() {
        let root = tempfile::tempdir().unwrap();
        let unit = unit();
        observe_held(root.path(), &unit, "schema_invalid", "input-a").unwrap();
        let record = read(root.path(), &unit.id().unwrap()).unwrap().unwrap();
        let future = [DeclaredFix {
            id: "future",
            reason: "schema_invalid",
            introduced: "9999.0.0",
        }];
        let unrelated = [DeclaredFix {
            id: "other",
            reason: "network",
            introduced: "2.0.0",
        }];
        assert!(record.permission("input-a", &future).is_none());
        assert!(record.permission("input-a", &unrelated).is_none());
        let selected = record.permission("input-a", FIXES).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let threads = (0..4)
            .map(|_| {
                let journal = root.path().to_path_buf();
                let unit = unit.clone();
                let selected = selected.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    consume(
                        &journal,
                        &unit,
                        &selected,
                        "schema_invalid",
                        "input-a",
                        FIXES,
                    )
                    .is_ok()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            threads
                .into_iter()
                .map(|t| usize::from(t.join().unwrap()))
                .sum::<usize>(),
            1
        );
        assert!(
            read(root.path(), &unit.id().unwrap())
                .unwrap()
                .unwrap()
                .permission("input-a", FIXES)
                .is_none()
        );
    }

    #[test]
    fn failing_checkpoint_leaves_consumed_unset_and_finish_keeps_it() {
        let root = tempfile::tempdir().unwrap();
        let unit = unit();
        let id = unit.id().unwrap();
        observe_held(root.path(), &unit, "schema_invalid", "input-a").unwrap();
        request(root.path(), &unit).unwrap();
        let first = read(root.path(), &id)
            .unwrap()
            .unwrap()
            .permission("input-a", &[])
            .unwrap();
        assert!(
            consume_checked(
                root.path(),
                &unit,
                &first,
                "schema_invalid",
                "input-a",
                &[],
                &|| Err("stopped".into()),
            )
            .is_err()
        );
        assert!(
            !read(root.path(), &id)
                .unwrap()
                .unwrap()
                .consumed
                .contains(&first.cause)
        );
        consume_checked(
            root.path(),
            &unit,
            &first,
            "schema_invalid",
            "input-a",
            &[],
            &|| Ok(()),
        )
        .unwrap();
        assert!(
            consume_checked(
                root.path(),
                &unit,
                &first,
                "schema_invalid",
                "input-a",
                &[],
                &|| Ok(()),
            )
            .is_err()
        );
        assert!(
            finish_checked(root.path(), &unit, &first, Some("schema_invalid"), &|| Err(
                "stopped".into()
            ),)
            .is_err()
        );
        let held = read(root.path(), &id).unwrap().unwrap();
        assert!(held.consumed.contains(&first.cause));
        assert!(held.attempt.is_some());
        finish_checked(root.path(), &unit, &first, Some("schema_invalid"), &|| {
            Ok(())
        })
        .unwrap();
        observe_held_checked(root.path(), &unit, "schema_invalid", "input-a", &|| Ok(())).unwrap();
        let restored = read(root.path(), &id).unwrap().unwrap();
        assert!(restored.consumed.contains(&first.cause));
        assert!(restored.held);
        assert!(restored.attempt.is_none());
    }
}
