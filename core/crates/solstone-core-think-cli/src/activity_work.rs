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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockedDisposition {
    pub reason_code: String,
    pub message: String,
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
    #[serde(default)]
    reasons: BTreeMap<String, String>,
    attempts: u32,
    next_attempt_ms: i64,
    #[serde(default)]
    destination_id: Option<String>,
    #[serde(default)]
    muted_pause: bool,
    #[serde(default)]
    blocked: Option<BlockedDisposition>,
}

pub(crate) struct ActivityWork {
    path: PathBuf,
    record: Record,
    _claim: FileLock,
    cancelled: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
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
    use solstone_core_journal_io::durability::{
        ArtifactId, DurableObservation, observe_json_durable,
    };
    match observe_json_durable::<Record>(ArtifactId::ActivityWork, path) {
        DurableObservation::Present(record) if record.version == 1 => Ok(Some(record)),
        DurableObservation::Present(_) => Err("unsupported activity work version".to_owned()),
        DurableObservation::Absent => Ok(None),
        DurableObservation::Malformed { path, .. } => {
            Err(format!("malformed activity work: {}", path.display()))
        }
        DurableObservation::Unreadable { path, source } => Err(format!(
            "cannot read activity work {}: {source}",
            path.display()
        )),
    }
}

/// The error a run gets when another run holds the same activity's work.
pub(crate) const CLAIMED: &str = "activity work is held by another run";

fn claim(context: &ThinkContext, identity: &ActivityRetry) -> Result<FileLock, String> {
    let claim_path = crate::segment::activity_provenance_path(
        context,
        &identity.day,
        &identity.facet,
        &identity.activity,
    );
    std::fs::create_dir_all(claim_path.parent().expect("provenance parent"))
        .map_err(|e| e.to_string())?;
    hold_lock(
        &claim_path,
        LockOptions {
            timeout: Duration::ZERO,
            ..LockOptions::default()
        },
    )
    .map_err(|error| match error {
        solstone_core_journal_io::LockError::Timeout(_) => CLAIMED.to_owned(),
        error => error.to_string(),
    })
}

/// Reconciliation starts only new work. A retained record, including an empty
/// record between publication and cleanup, belongs to its ordinary retry owner.
pub(crate) fn claim_drained(
    context: &ThinkContext,
    facet: &str,
    activity: &str,
) -> Result<FileLock, String> {
    let identity = ActivityRetry {
        day: context.day.clone(),
        facet: facet.into(),
        activity: activity.into(),
    };
    let held = claim(context, &identity)?;
    context.checkpoint()?;
    let root =
        solstone_core_journal_io::JournalRoot::open(&context.journal).map_err(|e| e.to_string())?;
    let work_path = path(&context.journal, &identity)?;
    let relative = work_path
        .strip_prefix(&context.journal)
        .map_err(|e| e.to_string())?;
    // Presence alone holds this boundary, including malformed/empty records.
    // Do not follow a separate pathname read after the bounded day census.
    if solstone_core_journal_io::read_relative_file_bounded(&root, relative, 4 * 1024 * 1024)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("retained activity has unfinished work".into());
    }
    Ok(held)
}

pub(crate) fn has_persisted_disposition(
    journal: &Path,
    day: &str,
    facet: &str,
    activity: &str,
) -> Result<bool, String> {
    let identity = ActivityRetry {
        day: day.to_owned(),
        facet: facet.to_owned(),
        activity: activity.to_owned(),
    };
    Ok(read(&path(journal, &identity)?)?
        .is_some_and(|record| record.blocked.is_some() || record.muted_pause))
}

/// A connection failure never exhausts its retries. Only the delay is capped.
fn backoff_ms(attempts: u32) -> i64 {
    (60_000_i64 * (1_i64 << attempts.saturating_sub(1).min(6))).min(3_600_000)
}

impl ActivityWork {
    pub(crate) fn open_existing(
        context: &ThinkContext,
        facet: &str,
        activity: &str,
    ) -> Result<Option<Self>, String> {
        let identity = ActivityRetry {
            day: context.day.clone(),
            facet: facet.to_owned(),
            activity: activity.to_owned(),
        };
        let path = path(&context.journal, &identity)?;
        let claim = claim(context, &identity)?;
        context.checkpoint()?;
        let Some(record) = read(&path)? else {
            return Ok(None);
        };
        if record.identity != identity {
            return Err("activity work identity mismatch".to_owned());
        }
        Ok(Some(Self {
            path,
            record,
            _claim: claim,
            cancelled: std::sync::Arc::clone(&context.cancel_checkpoint),
        }))
    }
    pub(crate) fn begin(
        context: &ThinkContext,
        facet: &str,
        activity: &str,
        input_hash: String,
        names: BTreeSet<String>,
        refresh: bool,
    ) -> Result<Self, String> {
        Self::begin_held(context, facet, activity, input_hash, names, refresh, None)
    }

    /// [`Self::begin`], keeping a claim this run already holds for the same
    /// activity instead of taking it again.
    pub(crate) fn begin_held(
        context: &ThinkContext,
        facet: &str,
        activity: &str,
        input_hash: String,
        names: BTreeSet<String>,
        refresh: bool,
        held: Option<FileLock>,
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
        let claim = match held {
            Some(held) => held,
            None => claim(context, &identity)?,
        };
        context.checkpoint()?;
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
            Some(mut record)
                if !refresh && (record.input_hash == input_hash || !record.parked.is_empty()) =>
            {
                if record.identity != identity {
                    return Err("activity work identity mismatch".to_owned());
                }
                // Disabled/removed talents stop being eligible; completed siblings
                // stay absent. A new input revision starts a fresh unit set.
                record.remaining.retain(|name| names.contains(name));
                if context.retry.is_some() {
                    record.input_hash = input_hash;
                }
                record
            }
            _ => Record {
                version: 1,
                identity,
                input_hash,
                remaining: names,
                uses: BTreeMap::new(),
                parked: BTreeSet::new(),
                reasons: BTreeMap::new(),
                attempts: 0,
                next_attempt_ms: 0,
                destination_id: None,
                muted_pause: false,
                blocked: None,
            },
        };
        let work = Self {
            path,
            record,
            _claim: claim,
            cancelled: std::sync::Arc::clone(&context.cancel_checkpoint),
        };
        work.save()?;
        Ok(work)
    }

    /// Give up the record but keep the claim, so no other run starts this
    /// activity's work before the holder runs it.
    pub(crate) fn into_claim(self) -> FileLock {
        self._claim
    }
    pub(crate) fn is_complete(&self) -> bool {
        self.record.remaining.is_empty()
    }
    pub(crate) fn due(&self, now_ms: i64) -> bool {
        if self.record.blocked.is_some() || self.record.muted_pause {
            return false;
        }
        self.record.remaining.is_empty() || self.record.next_attempt_ms <= now_ms
    }
    pub(crate) fn is_blocked(&self) -> bool {
        self.record.blocked.is_some()
    }
    pub(crate) fn blocked_disposition(&self) -> Option<&BlockedDisposition> {
        self.record.blocked.as_ref()
    }
    pub(crate) fn is_muted_pause(&self) -> bool {
        self.record.muted_pause
    }
    pub(crate) fn destination_id(&self) -> Option<&str> {
        self.record.destination_id.as_deref()
    }
    pub(crate) fn set_destination_id(&mut self, id: String) -> Result<(), String> {
        self.record.destination_id = Some(id);
        self.save()
    }
    pub(crate) fn block(&mut self, reason_code: &str, message: &str) -> Result<(), String> {
        self.record.blocked = Some(BlockedDisposition {
            reason_code: reason_code.to_owned(),
            message: message.to_owned(),
        });
        self.save()
    }
    pub(crate) fn set_muted_pause(&mut self, paused: bool) -> Result<(), String> {
        self.record.muted_pause = paused;
        self.save()
    }
    pub(crate) fn reactivate(&mut self) -> Result<(), String> {
        self.record.blocked = None;
        self.record.muted_pause = false;
        self.record.next_attempt_ms = 0;
        self.save()
    }
    pub(crate) fn parked(&self, name: &str) -> bool {
        self.record.parked.contains(name)
    }
    pub(crate) fn park(
        &mut self,
        name: &str,
        reason: &str,
        context: &ThinkContext,
    ) -> Result<(), String> {
        self.record.parked.insert(name.to_owned());
        self.record.reasons.insert(name.into(), reason.into());
        solstone_core_system::work_retry::observe_held_checked(
            &context.journal,
            &retry_unit(&self.record.identity, name),
            reason,
            &self.record.input_hash,
            &|| context.checkpoint(),
        )?;
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
        let journal = self
            .path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .ok_or("activity work has no journal root")?;
        let unit = retry_unit(&self.record.identity, name);
        let _execution = solstone_core_system::work_retry::execution_claim(journal, &unit)?;
        solstone_core_system::work_retry::reconcile_accepted_checked(journal, &unit, &|| {
            if (self.cancelled)() {
                Err("thinking cancelled".into())
            } else {
                Ok(())
            }
        })?;
        self.record.remaining.remove(name);
        self.record.uses.remove(name);
        self.record.parked.remove(name);
        self.record.reasons.remove(name);
        self.save()
    }
    pub(crate) fn finish(&mut self, context: &ThinkContext) -> Result<(), String> {
        if (context.cancel_checkpoint)() {
            return Err("cancelled before publication".into());
        }
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
            context.checkpoint()?;
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
        self.save_checked(&|| {
            if (self.cancelled)() {
                Err("thinking cancelled".into())
            } else {
                Ok(())
            }
        })
    }
    fn save_checked(&self, checkpoint: &dyn Fn() -> Result<(), String>) -> Result<(), String> {
        checkpoint()?;
        let bytes = serde_json::to_vec(&self.record).map_err(|e| e.to_string())?;
        checkpoint()?;
        atomic_replace(&self.path, &bytes, AtomicWriteOptions::default()).map_err(|e| e.to_string())
    }
}

/// Read-only discovery has no date horizon: an interrupted older activity stays
/// eligible until it succeeds, even after restart or a prolonged outage.
fn retry_unit(identity: &ActivityRetry, talent: &str) -> solstone_core_system::work_retry::Unit {
    solstone_core_system::work_retry::Unit::Activity {
        day: identity.day.clone(),
        facet: identity.facet.clone(),
        activity: identity.activity.clone(),
        talent: talent.into(),
    }
}

fn legacy_retry_baseline(journal: &Path, record: &Record) -> Result<String, String> {
    use solstone_core_system::work_retry::input;
    let Some(activity) = solstone_core_facets::get_activity_record(
        journal,
        &record.identity.facet,
        &record.identity.day,
        &record.identity.activity,
    )
    .map_err(|e| e.to_string())?
    else {
        return Ok(record.input_hash.clone());
    };
    if input::legacy_activity_hash(journal, &record.identity.day, &activity).as_deref()
        == Some(&record.input_hash)
    {
        input::activity_hash(journal, &record.identity.day, &activity)
            .ok_or("activity input cannot be read".into())
    } else {
        Ok(record.input_hash.clone())
    }
}

pub fn activity_held_input(
    journal: &Path,
    unit: &solstone_core_system::work_retry::Unit,
) -> Result<Option<String>, String> {
    use solstone_core_system::work_retry::{self, Unit};
    let Unit::Activity {
        day,
        facet,
        activity,
        talent,
    } = unit
    else {
        return Err("not activity work".into());
    };
    let identity = ActivityRetry {
        day: day.clone(),
        facet: facet.clone(),
        activity: activity.clone(),
    };
    let Some(record) = read(&path(journal, &identity)?)? else {
        return Ok(None);
    };
    if record.identity != identity {
        return Err("activity work identity mismatch".into());
    }
    if !record.remaining.contains(talent) || !record.parked.contains(talent) {
        return Ok(None);
    }
    work_retry::observe_held(
        journal,
        unit,
        record
            .reasons
            .get(talent)
            .map(String::as_str)
            .unwrap_or("activity_failed"),
        &legacy_retry_baseline(journal, &record)?,
    )?;
    if let Some(blocked) = record.blocked {
        return Err(blocked.message);
    }
    if record.muted_pause {
        return Err("the activity's facet is muted".into());
    }
    work_retry::input::current(journal, unit).map(Some)
}

pub fn discover_held_activities(
    journal: &Path,
) -> (Vec<solstone_core_system::work_retry::Unit>, Vec<String>) {
    let entries = match std::fs::read_dir(directory(journal)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (vec![], vec![]),
        Err(e) => return (vec![], vec![e.to_string()]),
    };
    let mut units = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        let result = (|| -> Result<(), String> {
            let entry_path = entry.map_err(|e| e.to_string())?.path();
            if entry_path.extension().and_then(|s| s.to_str()) != Some("json") {
                return Ok(());
            }
            let Some(record) = read(&entry_path)? else {
                return Ok(());
            };
            if path(journal, &record.identity)? != entry_path {
                return Err("activity work filename mismatch".into());
            }
            for talent in record.parked.intersection(&record.remaining) {
                let unit = retry_unit(&record.identity, talent);
                unit.validate()?;
                if let Some(id) = record.uses.get(talent)
                    && solstone_core_cortex_client::get_use_end_state(journal, id)
                        .map_err(|e| e.to_string())?
                        == solstone_core_cortex_client::UseEndState::Finish
                {
                    continue;
                }
                let baseline = legacy_retry_baseline(journal, &record)?;
                solstone_core_system::work_retry::observe_held(
                    journal,
                    &unit,
                    record
                        .reasons
                        .get(talent)
                        .map(String::as_str)
                        .unwrap_or("activity_failed"),
                    &baseline,
                )?;
                units.push(unit);
            }
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(error);
        }
    }
    (units, errors)
}

pub fn due_activity_retries(journal: &Path, now_ms: i64) -> Result<Vec<ActivityRetry>, String> {
    let (due, errors) = discover_activity_retries(journal, now_ms);
    if errors.is_empty() {
        Ok(due)
    } else {
        Err(errors.join("; "))
    }
}

/// A rebuild seal cannot hide parked, blocked, muted or delayed activity work.
pub(crate) fn require_day_drained(journal: &Path, day: &str) -> Result<(), String> {
    let entries =
        crate::daily_lifecycle::rebuild_files(journal, Path::new("health/activity-work"))?
            .unwrap_or_default();
    for (relative, bytes) in entries {
        if relative.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let record: Record = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if record.version != 1 || path(journal, &record.identity)? != journal.join(relative) {
            return Err("activity work filename mismatch".into());
        }
        if record.identity.day == day {
            return Err("requested day has unfinished activity work".into());
        }
    }
    Ok(())
}

pub(crate) fn rebuild_receipts(
    context: &ThinkContext,
    day: &str,
) -> Result<BTreeMap<String, String>, String> {
    require_day_drained(&context.journal, day)?;
    let mut proof = BTreeMap::new();
    for receipt in retained_receipts(context, day)? {
        let record = retained_record(context, day, &receipt.facet, &receipt.activity)?;
        if available_input(context, day, &record)? != receipt.input_hash {
            return Err("completed activity input changed".into());
        }
        proof.insert(receipt.proof_path, receipt.input_hash);
    }
    Ok(proof)
}

struct RetainedReceipt {
    path: PathBuf,
    bytes: Vec<u8>,
    proof_path: String,
    facet: String,
    activity: String,
    input_hash: String,
}

fn retained_receipts(context: &ThinkContext, day: &str) -> Result<Vec<RetainedReceipt>, String> {
    let relative = Path::new("chronicle")
        .join(day)
        .join("health/talent-provenance/activity-inputs");
    let entries =
        crate::daily_lifecycle::rebuild_files(&context.journal, &relative)?.unwrap_or_default();
    let mut receipts = Vec::new();
    for (path, bytes) in entries {
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let facet = value["facet"]
            .as_str()
            .ok_or("activity receipt has no facet")?;
        let activity = value["activity_id"]
            .as_str()
            .ok_or("activity receipt has no identity")?;
        let expected = value["input_hash"]
            .as_str()
            .ok_or("activity receipt has no input binding")?;
        if value["schema_version"] != 1
            || expected.len() != 64
            || !expected
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || crate::segment::activity_provenance_path(context, day, facet, activity)
                != context.journal.join(&path)
        {
            return Err("activity receipt identity changed".into());
        }
        // Validate components before either the facets reader or claim creates
        // ancestry. The provenance path alone uses encoded names.
        self::path(
            &context.journal,
            &ActivityRetry {
                day: day.into(),
                facet: facet.into(),
                activity: activity.into(),
            },
        )?;
        receipts.push(RetainedReceipt {
            proof_path: path
                .strip_prefix(&relative)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
            facet: facet.into(),
            activity: activity.into(),
            input_hash: expected.into(),
            path,
            bytes,
        });
    }
    Ok(receipts)
}

/// Parse the actual bounded no-follow bytes through the maintained strict row
/// projection. A malformed sibling must not disappear through tolerant parsing.
pub(crate) fn retained_record(
    context: &ThinkContext,
    day: &str,
    facet: &str,
    activity: &str,
) -> Result<solstone_core_facets::ActivityRecord, String> {
    let root =
        solstone_core_journal_io::JournalRoot::open(&context.journal).map_err(|e| e.to_string())?;
    let path = Path::new("facets")
        .join(facet)
        .join("activities")
        .join(format!("{day}.jsonl"));
    let read = || {
        solstone_core_journal_io::read_relative_file_bounded(&root, &path, 4 * 1024 * 1024)
            .map_err(|e| e.to_string())?
            .ok_or("completed activity disappeared".to_owned())
    };
    let before = read()?;
    context.checkpoint()?;
    let mut records = solstone_core_facets::parse_activity_records_strict(&before.bytes)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|r| r.get("id").and_then(serde_json::Value::as_str) == Some(activity));
    let record = records.next().ok_or("completed activity disappeared")?;
    if records.next().is_some() || before != read()? {
        return Err("retained activity identity changed".into());
    }
    if record.get("facet").and_then(serde_json::Value::as_str) != Some(facet) {
        return Err("retained activity identity changed".into());
    }
    root.revalidate().map_err(|e| e.to_string())?;
    Ok(record)
}

/// Complete changed retained inputs through their selected activity owner,
/// before the pure strict seal validator runs. Never use this on continuation.
pub(crate) fn reconcile_retained(
    context: &ThinkContext,
    log: &mut crate::run_log::RunLogWriter,
    max_concurrency: i64,
) -> Result<(), String> {
    require_day_drained(&context.journal, &context.day)?;
    let receipts = retained_receipts(context, &context.day)?;
    let expected = receipts
        .iter()
        .map(|receipt| receipt.proof_path.clone())
        .collect::<BTreeSet<_>>();
    for receipt in receipts {
        context.checkpoint()?;
        let held = claim_drained(context, &receipt.facet, &receipt.activity)?;
        let root = solstone_core_journal_io::JournalRoot::open(&context.journal)
            .map_err(|e| e.to_string())?;
        let current = solstone_core_journal_io::read_relative_file_bounded(
            &root,
            &receipt.path,
            4 * 1024 * 1024,
        )
        .map_err(|e| e.to_string())?
        .ok_or("activity receipt disappeared")?;
        if current.bytes != receipt.bytes {
            return Err("activity receipt identity changed".into());
        }
        let record = retained_record(context, &context.day, &receipt.facet, &receipt.activity)?;
        let hash = available_input(context, &context.day, &record)?;
        if hash == receipt.input_hash {
            continue;
        }
        if record
            .get("hidden")
            .is_some_and(solstone_core_facets::activity_value_truthy)
            || solstone_core_talent_runtime::activity_contract::is_synthetic(&record)
        {
            return Err("retained activity is held".into());
        }
        let outcome = crate::activity::reconcile_retained(
            context,
            log,
            &receipt.activity,
            &receipt.facet,
            &hash,
            max_concurrency,
            held,
        )?;
        if outcome.failed != 0 || outcome.timed_out {
            return Err("retained activity work did not complete".into());
        }
        context.checkpoint()?;
        let after = retained_record(context, &context.day, &receipt.facet, &receipt.activity)?;
        if after
            .get("hidden")
            .is_some_and(solstone_core_facets::activity_value_truthy)
            || available_input(context, &context.day, &after)? != hash
        {
            return Err("retained activity input changed during work".into());
        }
    }
    let current = rebuild_receipts(context, &context.day)?;
    if expected.iter().any(|path| !current.contains_key(path)) {
        return Err("retained activity receipt disappeared".into());
    }
    Ok(())
}

/// The normal fingerprint also represents absent Sense/segments. Admission
/// additionally requires every declared span to resolve uniquely to real input.
pub(crate) fn available_input(
    context: &ThinkContext,
    day: &str,
    record: &solstone_core_facets::ActivityRecord,
) -> Result<String, String> {
    solstone_core_talent_runtime::activity_contract::validated_span(record)
        .map_err(|_| "retained activity span is unavailable".to_owned())?;
    let input = solstone_core_system::work_retry::input::activity_input_bounded(
        &context.journal,
        day,
        record,
        || context.checkpoint(),
    )?;
    for sense in &input.sense {
        if !crate::segment::valid_activity_sense(sense) {
            return Err("retained activity Sense is malformed".into());
        }
    }
    Ok(input.hash)
}

/// A broken sibling is a visible gap, not a reason to suppress all valid work.
pub fn discover_activity_retries(journal: &Path, now_ms: i64) -> (Vec<ActivityRetry>, Vec<String>) {
    let entries = match std::fs::read_dir(directory(journal)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (vec![], vec![]),
        Err(e) => return (vec![], vec![e.to_string()]),
    };
    let mut due = Vec::new();
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
        if path.extension().and_then(|x| x.to_str()) != Some("json")
            || path
                .file_name()
                .and_then(|x| x.to_str())
                .is_some_and(|name| {
                    name.contains(solstone_core_journal_io::durability::SET_ASIDE_MARKER)
                })
        {
            continue;
        }
        match due_activity_record(journal, &path, now_ms) {
            Ok(Some(identity)) => due.push(identity),
            Ok(None) => {}
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    due.sort_by(|a, b| (&b.day, &a.facet, &a.activity).cmp(&(&a.day, &b.facet, &b.activity)));
    (due, errors)
}

fn due_activity_record(
    journal: &Path,
    entry: &Path,
    now_ms: i64,
) -> Result<Option<ActivityRetry>, String> {
    let Some(record) = read(entry)? else {
        return Ok(None);
    };
    if path(journal, &record.identity)? != entry {
        return Err("activity work filename mismatch".to_owned());
    }
    let has_finished_use = record
        .uses
        .iter()
        .filter(|(name, _)| record.remaining.contains(*name))
        .map(|(_, id)| {
            solstone_core_cortex_client::get_use_end_state(journal, id)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?
        .contains(&solstone_core_cortex_client::UseEndState::Finish);
    if record.blocked.is_some() && !has_finished_use {
        return Ok(None);
    }
    if record.muted_pause && !has_finished_use {
        let still_muted = match solstone_core_facets::observe_facet_destination(
            journal,
            &record.identity.facet,
        ) {
            Ok(solstone_core_facets::DestinationObservation::Ready { id, muted: true }) => record
                .destination_id
                .as_deref()
                .is_some_and(|stored| stored == id),
            _ => false,
        };
        if still_muted {
            return Ok(None);
        }
    }
    if has_finished_use
        || (record.next_attempt_ms <= now_ms
            && (record.remaining.is_empty()
                || record.remaining.iter().any(|n| !record.parked.contains(n))))
    {
        return Ok(Some(record.identity));
    }
    Ok(None)
}

pub fn activity_retry_due(
    journal: &Path,
    identity: &ActivityRetry,
    now_ms: i64,
) -> Result<bool, String> {
    Ok(due_activity_record(journal, &path(journal, identity)?, now_ms)?.as_ref() == Some(identity))
}

/// Adopt unfinished connection failures from a supplied recent source day on
/// upgrade. Existing work records are never replaced. Historical days are not
/// implicitly regenerated; durable records themselves have no age cutoff.
pub fn seed_activity_retries(journal: &Path, day: &str, now_ms: i64) -> Result<(), String> {
    seed_activity_retries_checked(journal, day, now_ms, &|| Ok(()))
}

pub(crate) fn seed_activity_retries_checked(
    journal: &Path,
    day: &str,
    now_ms: i64,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
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
        checkpoint()?;
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
            cancelled: std::sync::Arc::clone(&context.cancel_checkpoint),
            record: Record {
                version: 1,
                identity,
                input_hash,
                remaining: uses.keys().cloned().collect(),
                uses: uses.into_iter().filter(|(_, id)| !id.is_empty()).collect(),
                parked: BTreeSet::new(),
                reasons: BTreeMap::new(),
                destination_id: None,
                muted_pause: false,
                blocked: None,
                attempts: 0,
                next_attempt_ms: 0,
            },
        }
        .save_checked(checkpoint)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoDispatch;
    impl crate::context::CortexBoundary for NoDispatch {
        fn dispatch_prepared(
            &self,
            _: &tokio::runtime::Runtime,
            _: &solstone_core_cortex_client::CortexRequest,
            _: Option<&str>,
            _: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
        ) -> Result<String, crate::context::DispatchFailure> {
            panic!("held input must not dispatch");
        }
        fn dispatch(
            &self,
            _: &tokio::runtime::Runtime,
            _: &solstone_core_cortex_client::CortexRequest,
        ) -> Result<String, crate::context::DispatchFailure> {
            panic!("held input must not dispatch");
        }
        fn wait(
            &self,
            _: &tokio::runtime::Runtime,
            _: &[String],
            _: Option<Duration>,
        ) -> Result<solstone_core_cortex_client::WaitForUsesReport, String> {
            panic!("held input must not wait");
        }
    }

    fn retained_fixture(root: &Path) -> (ThinkContext, PathBuf, PathBuf) {
        let day = "20261016";
        let segment = root.join("chronicle").join(day).join("notes/090000_300");
        std::fs::create_dir_all(segment.join("talents")).unwrap();
        std::fs::write(
            segment.join("audio.jsonl"),
            "{\"start\":\"00:00:01\",\"text\":\"Declared synthetic garden note.\"}\n",
        )
        .unwrap();
        std::fs::write(
            segment.join("talents/sense.json"),
            r#"{"density":"active","content_type":"writing","facets":[]}"#,
        )
        .unwrap();
        solstone_core_facets::create_facet(root, "home", "Home", "Test", "#123456", "", None)
            .unwrap();
        let record = serde_json::json!({"id":"writing_090000_300","activity":"writing","facet":"home","stream":"notes","segments":["090000_300"]}).as_object().unwrap().clone();
        let _ = solstone_core_facets::append_activity_record(root, "home", day, record.clone())
            .unwrap();
        let payload = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone");
        let context = ThinkContext::new(
            root,
            day.into(),
            root.join("chronicle").join(day),
            chrono::Utc::now().timestamp_millis(),
        )
        .unwrap()
        .with_talent_roots(payload.join("talent"), payload.join("apps"))
        .with_boundary(std::sync::Arc::new(NoDispatch));
        let hash = crate::segment::compute_activity_input_hash(&context, day, &record).unwrap();
        crate::segment::write_activity_provenance(
            &context,
            day,
            "home",
            "writing_090000_300",
            &hash,
        )
        .unwrap();
        let receipt =
            crate::segment::activity_provenance_path(&context, day, "home", "writing_090000_300");
        // The retained input is stale; only actual owning work may replace it.
        std::fs::write(
            segment.join("talents/sense.json"),
            r#"{"density":"active","content_type":"reading","facets":[]}"#,
        )
        .unwrap();
        (context, segment, receipt)
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn strict_retained_rows_consume_admitted_bytes_and_refuse_replacement() {
        for shape in ["replace", "grow", "alias", "fifo"] {
            let root = tempfile::tempdir().unwrap();
            let (mut context, _, _) = retained_fixture(root.path());
            let path = root.path().join("facets/home/activities/20261016.jsonl");
            let original = std::fs::read(&path).unwrap();
            let admitted = solstone_core_facets::parse_activity_records_strict(&original).unwrap();
            let changed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let callback_changed = changed.clone();
            let callback_path = path.clone();
            context.cancel_checkpoint = std::sync::Arc::new(move || {
                if !callback_changed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    if shape == "grow" {
                        std::fs::write(&callback_path, vec![b' '; 4 * 1024 * 1024 + 1]).unwrap();
                    } else {
                        std::fs::rename(&callback_path, callback_path.with_extension("original"))
                            .unwrap();
                        match shape {
                            "replace" => std::fs::write(&callback_path, &original).unwrap(),
                            "alias" => std::os::unix::fs::symlink(
                                callback_path.with_extension("original"),
                                &callback_path,
                            )
                            .unwrap(),
                            "fifo" => assert!(
                                std::process::Command::new("mkfifo")
                                    .arg(&callback_path)
                                    .status()
                                    .unwrap()
                                    .success()
                            ),
                            _ => unreachable!(),
                        }
                    }
                }
                false
            });
            // The checkpoint replaces the path after the admitted read. The
            // strict parser consumes that read's bytes; re-observation refuses.
            assert!(
                retained_record(&context, "20261016", "home", "writing_090000_300").is_err(),
                "{shape}"
            );
            assert!(changed.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(admitted.len(), 1);
        }
    }

    #[test]
    fn changed_input_never_replaces_any_existing_work_disposition() {
        for shape in [
            "pending",
            "running",
            "uncertain",
            "parked",
            "blocked",
            "muted",
            "empty",
        ] {
            let root = tempfile::tempdir().unwrap();
            let (context, _, receipt) = retained_fixture(root.path());
            let original = std::fs::read(&receipt).unwrap();
            let mut work = ActivityWork::begin(
                &context,
                "home",
                "writing_090000_300",
                "old-input".into(),
                BTreeSet::from(["work".into()]),
                false,
            )
            .unwrap();
            match shape {
                "running" | "uncertain" => {
                    work.record
                        .uses
                        .insert("work".into(), format!("retained-{shape}"));
                    if shape == "running" {
                        let dir = root.path().join("talents/work");
                        std::fs::create_dir_all(&dir).unwrap();
                        std::fs::write(
                            dir.join("retained-running_active.jsonl"),
                            b"{\"use_id\":\"retained-running\"}\n",
                        )
                        .unwrap();
                        assert_eq!(
                            solstone_core_cortex_client::use_file_status(
                                &root.path().join("talents"),
                                "retained-running"
                            )
                            .unwrap(),
                            solstone_core_cortex_client::UseFileStatus::Running
                        );
                    } else {
                        assert_eq!(
                            solstone_core_cortex_client::use_file_status(
                                &root.path().join("talents"),
                                "retained-uncertain"
                            )
                            .unwrap(),
                            solstone_core_cortex_client::UseFileStatus::NotFound
                        );
                    }
                }
                "parked" => {
                    work.record.parked.insert("work".into());
                }
                "blocked" => {
                    work.record.blocked = Some(BlockedDisposition {
                        reason_code: "retained".into(),
                        message: "held".into(),
                    });
                }
                "muted" => {
                    work.record.muted_pause = true;
                }
                "empty" => {
                    work.record.remaining.clear();
                }
                _ => {}
            }
            work.save().unwrap();
            let work_path = work.path.clone();
            drop(work);
            let retained = std::fs::read(&work_path).unwrap();
            assert!(
                claim_drained(&context, "home", "writing_090000_300").is_err(),
                "{shape}"
            );
            let mut log =
                crate::run_log::RunLogWriter::open(root.path(), &context.day, "held-regression");
            assert!(
                reconcile_retained(&context, &mut log, 1).is_err(),
                "{shape}"
            );
            assert_eq!(std::fs::read(work_path).unwrap(), retained, "{shape}");
            assert_eq!(std::fs::read(receipt).unwrap(), original, "{shape}");
        }
    }

    #[test]
    fn unavailable_malformed_hidden_and_ineligible_inputs_cannot_refresh_provenance() {
        for shape in [
            "missing_segment",
            "missing_sense",
            "malformed_sense",
            "incomplete_sense",
            "malformed_receipt",
            "empty_receipt_hash",
            "invalid_receipt_hash",
            "changed_receipt",
            "owner_story",
            "owner_participation",
            "malformed_day",
            "hidden",
            "synthetic",
            "disabled",
            "cancelled",
            "muted_destination",
            "replaced_destination",
            "missing_declaration",
        ] {
            let root = tempfile::tempdir().unwrap();
            let (mut context, segment, receipt) = retained_fixture(root.path());
            let original = std::fs::read(&receipt).unwrap();
            match shape {
                "missing_segment" => {
                    std::fs::rename(&segment, root.path().join("retained-segment")).unwrap()
                }
                "missing_sense" => {
                    std::fs::remove_file(segment.join("talents/sense.json")).unwrap()
                }
                "malformed_sense" => {
                    std::fs::write(segment.join("talents/sense.json"), b"malformed").unwrap()
                }
                "incomplete_sense" => {
                    std::fs::write(segment.join("talents/sense.json"), b"{}").unwrap();
                }
                "malformed_receipt" => {
                    std::fs::write(&receipt, b"malformed").unwrap();
                }
                "empty_receipt_hash" | "invalid_receipt_hash" => {
                    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
                    value["input_hash"] =
                        serde_json::Value::from(if shape == "empty_receipt_hash" {
                            String::new()
                        } else {
                            "g".repeat(64)
                        });
                    std::fs::write(&receipt, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "changed_receipt" => {
                    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
                    value["activity_id"] = serde_json::Value::from("different-identity");
                    std::fs::write(&receipt, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "owner_story" | "owner_participation" => {
                    let patch = if shape == "owner_story" {
                        serde_json::json!({"story":{"talent":"work","body":"Owner-written story","topics":[],"confidence":1.0}})
                    } else {
                        serde_json::json!({"participation":[]})
                    };
                    solstone_core_facets::update_activity_record(
                        root.path(),
                        "home",
                        &context.day,
                        "writing_090000_300",
                        patch.as_object().unwrap(),
                        "owner:test",
                        "Preserve owned output",
                        "2026-10-16T12:00:00Z",
                    )
                    .unwrap();
                }
                "malformed_day" => {
                    use std::io::Write;
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(root.path().join("facets/home/activities/20261016.jsonl"))
                        .unwrap();
                    file.write_all(b"malformed retained sibling\n").unwrap();
                }
                "hidden" | "synthetic" => {
                    let patch = if shape == "hidden" {
                        serde_json::json!({"hidden":true})
                    } else {
                        serde_json::json!({"source":"anticipated"})
                    };
                    solstone_core_facets::update_activity_record(
                        root.path(),
                        "home",
                        &context.day,
                        "writing_090000_300",
                        patch.as_object().unwrap(),
                        "owner:test",
                        "hold",
                        "2026-10-16T12:00:00Z",
                    )
                    .unwrap();
                }
                "disabled" => {
                    let payload = root.path().join("empty-payload");
                    std::fs::create_dir_all(payload.join("talent")).unwrap();
                    std::fs::create_dir_all(payload.join("apps")).unwrap();
                    context =
                        context.with_talent_roots(payload.join("talent"), payload.join("apps"));
                }
                "cancelled" => {
                    context.cancel_checkpoint = std::sync::Arc::new(|| true);
                }
                "muted_destination" => {
                    solstone_core_facets::create_facet(
                        root.path(),
                        "other",
                        "Other",
                        "Unrelated",
                        "#654321",
                        "",
                        None,
                    )
                    .unwrap();
                    solstone_core_facets::set_facet_muted(root.path(), "home", true).unwrap();
                }
                "replaced_destination" => {
                    solstone_core_facets::create_facet(
                        root.path(),
                        "other",
                        "Other",
                        "Replacement fixture",
                        "#654321",
                        "",
                        None,
                    )
                    .unwrap();
                    let file = root.path().join("facets/home/facet.json");
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
                    let replacement: serde_json::Value = serde_json::from_slice(
                        &std::fs::read(root.path().join("facets/other/facet.json")).unwrap(),
                    )
                    .unwrap();
                    value["id"] = replacement["id"].clone();
                    std::fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "missing_declaration" => {
                    std::fs::remove_file(root.path().join("facets/home/facet.json")).unwrap();
                }
                _ => unreachable!(),
            }
            let mut log =
                crate::run_log::RunLogWriter::open(root.path(), &context.day, "held-regression");
            let retained_receipt = std::fs::read(&receipt).unwrap();
            assert!(
                reconcile_retained(&context, &mut log, 1).is_err(),
                "{shape}"
            );
            assert_eq!(std::fs::read(receipt).unwrap(), retained_receipt, "{shape}");
        }
    }

    #[test]
    fn a_receipt_lost_after_inventory_is_not_recreated_by_reconciliation() {
        let root = tempfile::tempdir().unwrap();
        let (mut context, _, receipt) = retained_fixture(root.path());
        let path = receipt.clone();
        let deleted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = deleted.clone();
        context.cancel_checkpoint = std::sync::Arc::new(move || {
            if !flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
                std::fs::remove_file(&path).unwrap();
            }
            false
        });
        let mut log = crate::run_log::RunLogWriter::open(root.path(), &context.day, "lost-receipt");
        assert!(reconcile_retained(&context, &mut log, 1).is_err());
        assert!(deleted.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!receipt.exists());
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn rebuild_activity_observations_refuse_aliases_fifos_and_unbounded_receipts() {
        use std::os::unix::fs::symlink;
        let day = "20260101";
        for subtree in [
            "health/activity-work",
            "chronicle/20260101/health/talent-provenance/activity-inputs",
        ] {
            for case in ["directory_alias", "file_alias", "fifo", "bytes", "entries"] {
                let root = tempfile::tempdir().unwrap();
                let outside = tempfile::tempdir().unwrap();
                let context = ThinkContext::new(
                    root.path(),
                    day.into(),
                    root.path().join("chronicle").join(day),
                    1_700_000_000_000,
                )
                .unwrap();
                assert!(rebuild_receipts(&context, day).unwrap().is_empty());
                let base = root.path().join(subtree);
                std::fs::create_dir_all(base.parent().unwrap()).unwrap();
                if case == "directory_alias" {
                    symlink(outside.path(), &base).unwrap();
                } else {
                    std::fs::create_dir(&base).unwrap();
                    let file = base.join("receipt.json");
                    match case {
                        "file_alias" => {
                            let source = outside.path().join("receipt.json");
                            std::fs::write(&source, "{}").unwrap();
                            symlink(source, file).unwrap();
                        }
                        "fifo" => assert!(
                            std::process::Command::new("mkfifo")
                                .arg(&file)
                                .status()
                                .unwrap()
                                .success()
                        ),
                        "bytes" => std::fs::write(file, vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap(),
                        "entries" => {
                            for n in 0..1025 {
                                std::fs::write(base.join(format!("{n}.json")), "{}").unwrap();
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                assert!(
                    crate::daily_lifecycle::rebuild_files(root.path(), Path::new(subtree)).is_err(),
                    "bound observation must refuse before parsing: {subtree}/{case}"
                );
                assert!(rebuild_receipts(&context, day).is_err(), "{subtree}/{case}");
            }
        }
    }

    #[test]
    fn a_hidden_blocked_or_muted_activity_still_prevents_a_rebuild_seal() {
        for muted in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let identity = ActivityRetry {
                day: "20260101".into(),
                facet: "work".into(),
                activity: "meeting_120000_300".into(),
            };
            let target = path(root.path(), &identity).unwrap();
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            let record = Record {
                version: 1,
                identity,
                input_hash: "input".into(),
                remaining: BTreeSet::from(["activity".into()]),
                uses: BTreeMap::new(),
                parked: BTreeSet::from(["activity".into()]),
                reasons: BTreeMap::new(),
                attempts: 1,
                next_attempt_ms: i64::MAX,
                destination_id: None,
                muted_pause: muted,
                blocked: Some(BlockedDisposition {
                    reason_code: "held".into(),
                    message: "held".into(),
                }),
            };
            solstone_core_journal_io::write_json(
                &target,
                &record,
                solstone_core_journal_io::JsonWriteOptions::default(),
            )
            .unwrap();
            assert!(due_activity_retries(root.path(), 0).unwrap().is_empty());
            assert!(require_day_drained(root.path(), "20260101").is_err());
            assert!(require_day_drained(root.path(), "20260102").is_ok());
        }
    }
    #[test]
    fn connection_retries_back_off_without_exhaustion() {
        assert_eq!(backoff_ms(1), 60_000);
        assert_eq!(backoff_ms(2), 120_000);
        assert_eq!(backoff_ms(20), 3_600_000);
        assert_eq!(backoff_ms(u32::MAX), 3_600_000);
    }
}
