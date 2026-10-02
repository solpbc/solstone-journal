// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_journal_io::durability::{ArtifactId, DurableRead, read_json_durable};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_indexer::daily_evidence::{DailyEvidence, DayProjectionCache};
use solstone_core_journal_io::{DailyUnitIdentity, DailyUnitStatus, observe_daily_unit_record};
use solstone_core_talent_config::{
    TalentConfig, TalentFilter, load_talent_configs, read_talent_overrides,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageState {
    Current,
    CurrentDegraded,
    Outstanding,
    HistoricalUnverified,
    Unreadable,
}
impl CoverageState {
    /// An accepted result exists for the evidence that was observed.
    ///
    /// This is the publication question: may this unit's day certify, and did
    /// an attempt actually finish.  ⛔ It is not the question a backlog, a
    /// pending count or a reconciler is asking — those want [`Self::is_owed`].
    pub fn is_current(self) -> bool {
        matches!(self, Self::Current | Self::CurrentDegraded)
    }

    /// The owner is missing an output they should have.
    ///
    /// `HistoricalUnverified` is deliberately excluded: history that predates
    /// evidence-bound completion is readable and unverified by design, is
    /// outside the adoption boundary, and will never be regenerated on its
    /// own — reporting it as owed work is a standing false alarm for something
    /// nobody will ever act on.
    pub fn is_owed(self) -> bool {
        matches!(self, Self::Outstanding | Self::Unreadable)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnitCoverage {
    pub identity: DailyUnitIdentity,
    pub evidence_revision: String,
    pub contract_digest: String,
    pub state: CoverageState,
    pub reason_code: Option<String>,
    /// Set when the unit is current by an accepted result made under an earlier
    /// revision that it deliberately keeps ([`AcceptedReuse`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earlier_version: Option<AcceptedReuse>,
    /// Why an owed unit is owed: `never_made`, `evidence_changed`,
    /// `contract_changed`, `output_missing`, `retry`, `unconfirmed_write` or
    /// `conflict`.  Lets a release state what it would
    /// regenerate before it is installed (`journal reprocess --owed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owed_by: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DailyCoverage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance: Option<UnitCoverage>,
    pub day: String,
    pub as_of_ms: i64,
    pub state: CoverageState,
    pub units: Vec<UnitCoverage>,
}

pub fn package_roots() -> Result<(PathBuf, PathBuf), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = exe.parent().ok_or("executable has no parent")?;
    let root = solstone_core_journal::resolve_installation_root_from_executable_dir(directory)
        .ok_or_else(|| solstone_core_journal::describe_package_roots_miss(directory))?;
    Ok((root.join("solstone/talent"), root.join("solstone/apps")))
}

/// The owner's day (`YYYYMMDD`) at `now`, in the journal's owner zone.
pub fn local_day(journal: &Path, now: DateTime<Utc>) -> String {
    now.with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .format("%Y%m%d")
        .to_string()
}

pub fn daily_configs(
    journal: &Path,
    talent: &Path,
    apps: &Path,
) -> Result<Vec<TalentConfig>, String> {
    let overrides = read_talent_overrides(journal)?;
    load_talent_configs(
        talent,
        apps,
        overrides.as_ref(),
        TalentFilter {
            r#type: None,
            schedule: Some("daily"),
            include_disabled: false,
        },
    )
}

/// Orders whole-day coverage reads within this process: at most one runs at a time.
///
/// A coverage read is the heaviest allocation and free churn the supervisor does, and
/// each of the three known allocator aborts had two of them overlapping (the tick
/// thread's catch-up drain and a queue worker's completion check). The lock guards no
/// data; it only orders reads, so a poisoned lock is recovered.
///
/// It is a leaf lock. Nothing is acquired while it is held, so a holder never waits for
/// another lock. It is taken while holding the catch-up ledger lock (startup
/// reconciliation), and it must never be held while taking the ledger, adoption or
/// queue-state locks. The acquire blocks with no timeout: callers read an `Err` from a
/// coverage read as "not complete", so a timeout would record a finished catch-up as a
/// failed one.
static COVERAGE_READ: Mutex<()> = Mutex::new(());

/// Takes `lock` for one coverage read of `day`. A read that finds another in flight says
/// so before it waits, outside the lock, so a holder that never lets go shows as waiters'
/// lines. A poisoned lock is taken over: it guards nothing.
fn acquire_coverage_read<'a>(lock: &'a Mutex<()>, day: &str) -> MutexGuard<'a, ()> {
    match lock.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => {
            eprintln!("coverage read of {day} is waiting behind another coverage read");
            lock.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }
}

/// How many of the most recent closed days a contract change re-owes.
///
/// A contract is a talent's prompt, schema and templates, every facet
/// declaration and the active model.  By operator approval (2026-09-30), a change to it
/// re-derives the open day and the last seven closed days; older closed days
/// keep what they have, marked as made with an earlier version, and are
/// re-derived only when their own evidence changes or the owner asks
/// (`--from-scratch`).
pub const CONTRACT_REOWE_CLOSED_DAYS: i64 = 7;

/// Why an accepted result that does not match the current revision still counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptedReuse {
    /// A morning briefing whose morning has passed.  By operator approval (2026-09-30), it is
    /// a record of what the owner was told that morning and is never
    /// regenerated on its own, whatever changes later.
    FrozenBriefing,
    /// The same evidence under an older contract, on a closed day older than
    /// [`CONTRACT_REOWE_CLOSED_DAYS`].
    EarlierContract,
}

/// Whether `record` answers for this unit now.  `None` means it is owed.
///
/// `Some(None)` is an exact match on evidence and contract; `Some(Some(_))` is
/// an accepted result kept on purpose.  The caller still checks the accepted
/// artifacts exist.  ⛔ Every reader that decides reuse goes through this --
/// the coverage reader and the run's own skip must never disagree, or a day
/// reads current while its run regenerates it.
pub fn accepted_reuse(
    record: &solstone_core_journal_io::DailyUnitRecord,
    evidence: &DailyEvidence,
    today: &str,
) -> Option<Option<AcceptedReuse>> {
    if record.evidence_revision == solstone_core_journal_io::OWNER_REPROCESS_SENTINEL
        && record.contract_digest == solstone_core_journal_io::OWNER_REPROCESS_SENTINEL
    {
        return None;
    }
    if record.status.is_terminal_success()
        && record.is_reusable_for(&evidence.revision, &evidence.contract)
    {
        return Some(None);
    }
    // The global maintenance unit is keyed on today's window, never a closed day.
    if record.identity.name == "daily_schedule" {
        return None;
    }
    // What is kept is the accepted result, whatever a later attempt did: a
    // failed or interrupted re-run of a past briefing must not unfreeze it.
    // ⛔ But a conflict with the owner, or a write that may have landed, is
    // never kept -- it surfaces as owed.
    if record.status == DailyUnitStatus::Conflicting || record.has_uncommitted_started_receipt() {
        return None;
    }
    let accepted = record.accepted.as_ref()?;
    if !accepted.status.is_terminal_success() || !accepted.has_valid_proof() {
        return None;
    }
    // A newer attempt that already committed some of its owner writes must
    // resume; keeping the older result would strand what it published.
    let landed = |receipt: &Value| {
        receipt.get("kind").and_then(Value::as_str) == Some("owner_action")
            && !accepted.receipts.contains(receipt)
    };
    if !record.status.is_terminal_success() && record.receipts.iter().any(landed) {
        return None;
    }
    // Nothing has changed since the accepted result: a newer attempt for this
    // same revision (pending, capped, retried) governs, as it always has.
    if accepted.evidence_revision == evidence.revision
        && accepted.contract_digest == evidence.contract
    {
        return None;
    }
    let day = chrono::NaiveDate::parse_from_str(&record.identity.day, "%Y%m%d").ok()?;
    let today = chrono::NaiveDate::parse_from_str(today, "%Y%m%d").ok()?;
    // The briefing for day D is presented on the morning of D+1.
    if record.identity.name == "morning_briefing" && day + chrono::Duration::days(1) < today {
        return Some(Some(AcceptedReuse::FrozenBriefing));
    }
    if day < today - chrono::Duration::days(CONTRACT_REOWE_CLOSED_DAYS)
        && evidence.revision_under(&accepted.contract_digest) == accepted.evidence_revision
    {
        return Some(Some(AcceptedReuse::EarlierContract));
    }
    None
}

pub fn read_daily_coverage(journal: &Path, day: &str) -> Result<DailyCoverage, String> {
    let (talent, apps) = package_roots()?;
    read_daily_coverage_with_roots(journal, day, &talent, &apps)
}

pub fn read_daily_coverage_with_roots(
    journal: &Path,
    day: &str,
    talent: &Path,
    apps: &Path,
) -> Result<DailyCoverage, String> {
    read_daily_coverage_with_cache(
        journal,
        day,
        talent,
        apps,
        &mut DayProjectionCache::new(journal, day),
    )
}

/// One coverage read of `day`, sharing one whole-day source projection across its units.
fn read_daily_coverage_with_cache(
    journal: &Path,
    day: &str,
    talent: &Path,
    apps: &Path,
    cache: &mut DayProjectionCache,
) -> Result<DailyCoverage, String> {
    // Held for the whole read, and taken before any of its work. `read_unit_coverage`
    // is deliberately not guarded: the maintenance unit calls it from in here and the
    // lock is not reentrant.
    let _one_read_at_a_time = acquire_coverage_read(&COVERAGE_READ, day);
    let configs = daily_configs(journal, talent, apps)?;
    let facets =
        solstone_core_facets::list_declared_facet_names(journal).map_err(|e| e.to_string())?;
    let active = crate::activity_state::active_facets_checked(journal, day)?;
    let mut units = Vec::new();
    let mut maintenance = None;
    for config in configs {
        // Global maintenance is reported independently of historical evidence coverage.
        if config.key == "daily_schedule" {
            let today = local_day(journal, Utc::now());
            maintenance = Some(match read_unit_coverage(journal, &today, &config, None) {
                Ok(mut unit) => {
                    if unit.state == CoverageState::HistoricalUnverified {
                        unit.state = CoverageState::Outstanding;
                    }
                    unit
                }
                Err(error) => UnitCoverage {
                    identity: DailyUnitIdentity::new(&today, "daily_schedule", None),
                    evidence_revision: String::new(),
                    contract_digest: String::new(),
                    state: CoverageState::Unreadable,
                    reason_code: Some(error),
                    earlier_version: None,
                    owed_by: None,
                },
            });
            continue;
        }
        if config.metadata.get("multi_facet") == Some(&Value::Bool(true)) {
            for facet in &facets {
                if config.metadata.get("always") != Some(&Value::Bool(true))
                    && !active.contains(facet)
                {
                    continue;
                }
                units.push(read_unit_coverage_cached(
                    journal,
                    day,
                    &config,
                    Some(facet),
                    cache,
                )?);
            }
        } else {
            units.push(read_unit_coverage_cached(
                journal, day, &config, None, cache,
            )?);
        }
    }
    let state = if units.iter().any(|u| u.state == CoverageState::Unreadable) {
        CoverageState::Unreadable
    } else if units.iter().any(|u| u.state == CoverageState::Outstanding) {
        CoverageState::Outstanding
    } else if units
        .iter()
        .any(|u| u.state == CoverageState::HistoricalUnverified)
    {
        CoverageState::HistoricalUnverified
    } else if units
        .iter()
        .any(|u| u.state == CoverageState::CurrentDegraded)
    {
        CoverageState::CurrentDegraded
    } else {
        CoverageState::Current
    };
    Ok(DailyCoverage {
        maintenance,
        day: day.to_owned(),
        as_of_ms: Utc::now().timestamp_millis(),
        state,
        units,
    })
}

pub fn read_unit_coverage(
    journal: &Path,
    day: &str,
    config: &TalentConfig,
    facet: Option<&str>,
) -> Result<UnitCoverage, String> {
    read_unit_coverage_cached(
        journal,
        day,
        config,
        facet,
        &mut DayProjectionCache::new(journal, day),
    )
}

fn read_unit_coverage_cached(
    journal: &Path,
    day: &str,
    config: &TalentConfig,
    facet: Option<&str>,
    cache: &mut DayProjectionCache,
) -> Result<UnitCoverage, String> {
    let identity = DailyUnitIdentity::new(day, &config.key, facet.map(str::to_owned));
    let evidence = solstone_core_indexer::daily_evidence::compute_daily_evidence_cached(
        journal,
        day,
        &config.key,
        &config.metadata,
        &config.body,
        facet,
        None,
        cache,
    );
    let evidence = match evidence {
        Ok(evidence) => evidence,
        Err(error) if error.starts_with("unsupported") => {
            return Ok(UnitCoverage {
                identity,
                evidence_revision: String::new(),
                contract_digest: String::new(),
                state: CoverageState::HistoricalUnverified,
                reason_code: Some(error),
                earlier_version: None,
                owed_by: None,
            });
        }
        Err(error) => return Err(error),
    };
    let (e, contract) = (evidence.revision.clone(), evidence.contract.clone());
    let today = local_day(journal, Utc::now());
    let record = observe_daily_unit_record(journal, &identity).map_err(|e| e.to_string())?;
    let mut reason = None;
    let mut earlier_version = None;
    let state = match &record {
        None => {
            if day_is_adopted(journal, day)? {
                CoverageState::Outstanding
            } else {
                CoverageState::HistoricalUnverified
            }
        }
        Some(record) => {
            if record.evidence_revision == e && record.contract_digest == contract {
                reason = record.reason_code.clone();
            }
            if record.status == DailyUnitStatus::Conflicting
                || (identity.name == "entities:entities_review"
                    && record.has_uncommitted_started_receipt())
            {
                CoverageState::Outstanding
            } else if let Some(reuse) = accepted_reuse(record, &evidence, &today)
                // A kept result stands whatever became of its files: a deleted
                // or edited past output is the owner's, not work to redo.
                && (reuse.is_some()
                    || solstone_core_journal_io::accepted_daily_artifacts_valid(journal, record)
                        .map_err(|e| e.to_string())?)
            {
                earlier_version = reuse;
                if record
                    .accepted
                    .as_ref()
                    .and_then(|a| a.generated_result.as_ref())
                    .and_then(|r| r.get("degraded"))
                    .is_some_and(|value| !value.is_null())
                {
                    CoverageState::CurrentDegraded
                } else {
                    CoverageState::Current
                }
            } else if record.evidence_revision == e
                && record.contract_digest == contract
                && record.status == DailyUnitStatus::Capped
                && record
                    .reason_code
                    .as_deref()
                    .is_some_and(|reason| daily_failure_capped(reason, record.failure_count))
            {
                CoverageState::CurrentDegraded
            } else {
                CoverageState::Outstanding
            }
        }
    };
    Ok(UnitCoverage {
        identity,
        evidence_revision: e,
        contract_digest: contract,
        state,
        reason_code: reason,
        earlier_version,
        owed_by: if state.is_owed() {
            Some(owed_cause(journal, record.as_ref(), &evidence).to_owned())
        } else {
            None
        },
    })
}

/// Why an owed unit is owed, for `journal reprocess --owed`.
fn owed_cause(
    journal: &Path,
    record: Option<&solstone_core_journal_io::DailyUnitRecord>,
    evidence: &DailyEvidence,
) -> &'static str {
    let Some(record) = record else {
        return "never_made";
    };
    if record.status == DailyUnitStatus::Conflicting {
        return "conflict";
    }
    if record.has_uncommitted_started_receipt() {
        return "unconfirmed_write";
    }
    let Some(accepted) = record.accepted.as_ref() else {
        return "never_made";
    };
    // A diagnostic: it must never turn a readable day unreadable, so a result
    // whose proof or artifacts cannot be checked reads as not valid.
    let artifacts_valid = accepted.has_valid_proof()
        && solstone_core_journal_io::accepted_daily_artifacts_valid(journal, record)
            .unwrap_or(false);
    if accepted.evidence_revision == evidence.revision
        && accepted.contract_digest == evidence.contract
    {
        return if artifacts_valid {
            "retry"
        } else {
            "output_missing"
        };
    }
    if evidence.revision_under(&accepted.contract_digest) == accepted.evidence_revision {
        return if artifacts_valid {
            "contract_changed"
        } else {
            "output_missing"
        };
    }
    "evidence_changed"
}

pub fn environmental_failure(reason: &str) -> bool {
    matches!(
        reason,
        "model_not_found"
            | "provider_request_rejected"
            | "model_not_ready"
            | "provider_unavailable"
    )
}
pub fn daily_failure_capped(reason: &str, count: u32) -> bool {
    let cap = match reason {
        "model_not_found" | "provider_request_rejected" => 1,
        "schema_invalid" => 3,
        "agent_stuck"
        | "context_window_exceeded"
        | "max_turns_exhausted"
        | "no_output"
        | "non_responsive"
        | "token_budget_exceeded"
        | "wall_clock_exceeded" => 2,
        _ => return false,
    };
    count >= cap
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictRecovery {
    Open,
    StaleDeferred,
    Exhausted,
}

pub fn stale_receipt_free_conflict(record: &solstone_core_journal_io::DailyUnitRecord) -> bool {
    if record.status != DailyUnitStatus::Conflicting
        || record.reason_code.as_deref() != Some("daily_owner_conflict")
    {
        return false;
    }
    if record
        .receipts
        .iter()
        .any(|r| r.get("kind").and_then(Value::as_str) == Some("owner_action"))
    {
        return false;
    }
    match record.owner_conflict_kind.as_deref() {
        Some("merge_proposal_preparation" | "merge_proposals_changed" | "calendar_changed") => true,
        None => {
            record.identity.name == "schedule"
                && record.error_detail.as_deref().is_some_and(|detail| {
                    detail.contains("calendar changed after prompt preparation")
                })
        }
        _ => false,
    }
}

pub fn conflict_recovery(
    record: &solstone_core_journal_io::DailyUnitRecord,
    today: &str,
    journal: &Path,
) -> ConflictRecovery {
    if record.status != DailyUnitStatus::Conflicting || record.failure_count < 2 {
        return ConflictRecovery::Open;
    }
    if stale_receipt_free_conflict(record) {
        let charged_day = conflict_attempt_day(record, journal);
        if charged_day.as_deref() == Some(today) {
            ConflictRecovery::StaleDeferred
        } else if charged_day.as_deref().is_some_and(|day| day < today) {
            ConflictRecovery::Open
        } else {
            ConflictRecovery::Exhausted
        }
    } else {
        ConflictRecovery::Exhausted
    }
}

/// The local day charged by this attempt, including version-one legacy records.
pub fn conflict_attempt_day(
    record: &solstone_core_journal_io::DailyUnitRecord,
    journal: &Path,
) -> Option<String> {
    match record.attempt_day.as_deref() {
        Some(day) => chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
            .ok()
            .map(|_| day.to_owned()),
        None => chrono::DateTime::from_timestamp_millis(record.updated_at_ms)
            .map(|instant| local_day(journal, instant)),
    }
}

/// Adoption and reconciliation bookkeeping; accepted unit records remain the sole result authority.
#[derive(Default, Serialize, Deserialize)]
struct Adoption {
    version: u32,
    first_closed_day: String,
    adopted: std::collections::BTreeSet<String>,
    pending: std::collections::BTreeSet<String>,
    cursor: Option<String>,
}

fn day_is_adopted(journal: &Path, day: &str) -> Result<bool, String> {
    let path = journal.join("health/daily-adoption.json");
    // ⛔ Adoption is bookkeeping -- the accepted unit records are the sole result
    // authority (see `Adoption`). A file we cannot parse must read as "this day
    // was never adopted", not as an error: erroring here makes EVERY day's
    // coverage unreadable, which stops daily processing entirely and makes the
    // backlog, the doctor and reprocess all fail, for a file the next
    // reconciliation pass rebuilds on its own.
    let state: Adoption = match solstone_core_journal_io::durability::observe_json_durable(
        ArtifactId::DailyAdoption,
        &path,
    ) {
        solstone_core_journal_io::durability::DurableObservation::Present(state) => state,
        solstone_core_journal_io::durability::DurableObservation::Absent
        | solstone_core_journal_io::durability::DurableObservation::Malformed { .. }
        | solstone_core_journal_io::durability::DurableObservation::Unreadable { .. } => {
            return Ok(false);
        }
    };
    if state.version != 1 {
        return Ok(false);
    }
    Ok(state.adopted.contains(day))
}

/// Keep explicitly processed old days in the bounded reconciliation set, including failed preparation.
pub fn register_daily_day(journal: &Path, day: &str, now: DateTime<Utc>) -> Result<(), String> {
    use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};
    chrono::NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|e| e.to_string())?;
    let today = local_day(journal, now);
    let path = journal.join("health/daily-adoption.json");
    std::fs::create_dir_all(path.parent().expect("health parent")).map_err(|e| e.to_string())?;
    let _lock = hold_lock(path.with_extension("lock"), LockOptions::default())
        .map_err(|e| e.to_string())?;
    let mut state = load_adoption(&path, &today)?;
    state.adopted.insert(day.to_owned());
    state.pending.insert(day.to_owned());
    write_json(&path, &state, JsonWriteOptions::default()).map_err(|e| e.to_string())
}

fn load_adoption(path: &Path, today: &str) -> Result<Adoption, String> {
    let date = chrono::NaiveDate::parse_from_str(today, "%Y%m%d").map_err(|e| e.to_string())?;
    // A pointer we cannot parse is rebuilt from the same defaults an absent one
    // gets.  ⚠ The cost of rebuilding is at most one redundant reconciliation
    // pass; the cost of refusing is that no day is ever processed again.
    let fresh = || Adoption {
        version: 1,
        first_closed_day: (date - chrono::Duration::days(7))
            .format("%Y%m%d")
            .to_string(),
        ..Adoption::default()
    };
    let state: Adoption = match read_json_durable(ArtifactId::DailyAdoption, path) {
        Ok(DurableRead::Present(state)) => state,
        Ok(DurableRead::Absent | DurableRead::SetAside(_) | DurableRead::Unreadable { .. }) => {
            fresh()
        }
        Err(e) => return Err(e.to_string()),
    };
    if state.version != 1 {
        return Ok(fresh());
    }
    Ok(state)
}

pub fn reconcile_days(
    journal: &Path,
    explicit: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<String>, String> {
    let (talent, apps) = package_roots()?;
    reconcile_days_with_roots(journal, explicit, now, &talent, &apps)
}

pub fn reconcile_days_with_roots(
    journal: &Path,
    explicit: &[String],
    now: DateTime<Utc>,
    talent: &Path,
    apps: &Path,
) -> Result<Vec<String>, String> {
    use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};
    let today = local_day(journal, now);
    let path = journal.join("health/daily-adoption.json");
    std::fs::create_dir_all(path.parent().expect("health parent")).map_err(|e| e.to_string())?;
    let lock_path = path.with_extension("lock");
    let days = solstone_core_journal_io::day_dirs(journal).map_err(|e| e.to_string())?;

    // Phase 1, under the lock: adopt days and choose this pass's batch.  The
    // lock covers only the read-modify-write of the adoption file.
    let (selected, adopted_at_selection) = {
        let _lock = hold_lock(&lock_path, LockOptions::default()).map_err(|e| e.to_string())?;
        let mut state = load_adoption(&path, &today)?;
        for day in days.keys() {
            if day >= &today {
                continue;
            }
            let known_dirty = raw_marker_dirty(journal, day)?;
            if (day >= &state.first_closed_day || explicit.contains(day) || known_dirty)
                && (state.adopted.insert(day.clone()) || known_dirty)
            {
                state.pending.insert(day.clone());
            }
        }
        for day in explicit {
            if days.contains_key(day) {
                state.adopted.insert(day.clone());
                state.pending.insert(day.clone());
            }
        }
        let candidates = state
            .adopted
            .iter()
            .filter(|day| day.as_str() < today.as_str())
            .cloned()
            .collect::<Vec<_>>();
        let start = state
            .cursor
            .as_ref()
            .map(|cursor| candidates.partition_point(|day| day <= cursor))
            .unwrap_or(0);
        let selected = candidates
            .iter()
            .skip(start)
            .chain(candidates.iter().take(start))
            .take(4)
            .cloned()
            .collect::<Vec<_>>();
        write_json(&path, &state, JsonWriteOptions::default()).map_err(|e| e.to_string())?;
        (selected, state.adopted)
    };

    // Phase 2, unlocked: coverage is the most expensive read in this module and
    // it mutates nothing.  Holding the adoption lock across it made every
    // concurrent `register_daily_day` race a 10s timeout.
    let mut decisions = Vec::with_capacity(selected.len());
    for day in selected {
        let still_pending = day_still_pending(journal, &day, talent, apps, &today)?;
        decisions.push((day, still_pending));
    }

    // Phase 3, under the lock again: re-read before writing.  ⛔ Never write
    // back the phase-1 struct — a day registered while phase 2 ran is in the
    // file and not in that copy, and restoring it would be a lost update whose
    // symptom is a registered day that is never processed.
    let _lock = hold_lock(&lock_path, LockOptions::default()).map_err(|e| e.to_string())?;
    let mut state = load_adoption(&path, &today)?;
    // `register_daily_day` inserts into `adopted` and `pending` together, so a
    // grown adopted set is the signal that a day was registered while phase 2
    // ran.  Settling a day on a coverage reading taken before that would erase
    // the registration, and its symptom is a registered day that is never
    // processed.  Deferring the removals by one pass cannot lose work; applying
    // a stale one can.
    let registered_during_pass = state.adopted != adopted_at_selection;
    for (day, still_pending) in decisions {
        if still_pending {
            state.pending.insert(day.clone());
        } else if !registered_during_pass {
            state.pending.remove(&day);
        }
        state.cursor = Some(day);
    }
    write_json(&path, &state, JsonWriteOptions::default()).map_err(|e| e.to_string())?;
    Ok(state
        .pending
        .into_iter()
        .filter(|day| day < &today)
        .collect())
}

/// Whether `day` still has daily work owed: the one decision that takes a day
/// out of `pending`, shared by the reconciler and a run settling its own day.
fn day_still_pending(
    journal: &Path,
    day: &str,
    talent: &Path,
    apps: &Path,
    today: &str,
) -> Result<bool, String> {
    Ok(
        match read_daily_coverage_with_roots(journal, day, talent, apps) {
            Ok(coverage) => {
                let retry_due = coverage.units.iter().any(|unit| {
                    unit.state == CoverageState::CurrentDegraded
                        && unit
                            .reason_code
                            .as_deref()
                            .is_some_and(environmental_failure)
                        && observe_daily_unit_record(journal, &unit.identity)
                            .ok()
                            .flatten()
                            .is_some_and(|record| {
                                record.environmental_retry_day.as_deref() != Some(today)
                            })
                });
                coverage.state.is_owed() || retry_due || raw_marker_dirty(journal, day)?
            }
            Err(_) => true,
        },
    )
}

/// Take a day its own run has just brought current out of `pending`.
///
/// Every `think --day` registers its day ([`register_daily_day`]), which puts it
/// back in `pending`, and before this only the reconciler's rotating cursor --
/// four adopted days a pass -- could take it out again.  Until the cursor came
/// round, every drain pass resubmitted the day with nothing owed: up to 13 runs
/// a night for one day on a real journal, each with its whole-journal
/// phases.  A day that still owes work, or whose raw input moved, stays.
///
/// ⚠ If another day was registered while the coverage was read, this defers
/// to the reconciler rather than write over that registration.
pub fn settle_daily_day(
    journal: &Path,
    day: &str,
    talent: &Path,
    apps: &Path,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};
    let today = local_day(journal, now);
    if day >= today.as_str() {
        return Ok(false);
    }
    let path = journal.join("health/daily-adoption.json");
    let lock_path = path.with_extension("lock");
    let adopted_before = {
        let _lock = hold_lock(&lock_path, LockOptions::default()).map_err(|e| e.to_string())?;
        let state = load_adoption(&path, &today)?;
        if !state.pending.contains(day) {
            return Ok(false);
        }
        state.adopted
    };
    if day_still_pending(journal, day, talent, apps, &today)? {
        return Ok(false);
    }
    let _lock = hold_lock(&lock_path, LockOptions::default()).map_err(|e| e.to_string())?;
    let mut state = load_adoption(&path, &today)?;
    if state.adopted != adopted_before || !state.pending.remove(day) {
        return Ok(false);
    }
    write_json(&path, &state, JsonWriteOptions::default()).map_err(|e| e.to_string())?;
    Ok(true)
}

fn raw_marker_dirty(journal: &Path, day: &str) -> Result<bool, String> {
    #[cfg(unix)]
    {
        use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};
        let stream = read_health_marker(journal, day, HealthMarkerKind::Stream)
            .map_err(|e| e.to_string())?;
        let daily =
            read_health_marker(journal, day, HealthMarkerKind::Daily).map_err(|e| e.to_string())?;
        Ok(match (stream, daily) {
            (
                HealthMarkerState::Versioned { marker: stream, .. },
                HealthMarkerState::Versioned { marker: daily, .. },
            ) => stream.generation > daily.generation,
            (HealthMarkerState::Versioned { marker, .. }, _) => marker.generation > 0,
            _ => false,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (journal, day);
        Ok(false)
    }
}

/// Marker/queue tests explicitly configure no daily workloads so they isolate raw-input guards.
#[cfg(test)]
pub(crate) fn configure_no_daily_work(journal: &Path) {
    let (talent, apps) = package_roots().unwrap();
    let overrides = daily_configs(journal, &talent, &apps)
        .unwrap()
        .into_iter()
        .map(|config| {
            (
                solstone_core_talent_config::context_key(&config.key),
                serde_json::json!({"disabled": true}),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    std::fs::create_dir_all(journal.join("config")).unwrap();
    std::fs::write(
        journal.join("config/journal.json"),
        serde_json::to_vec(
            &serde_json::json!({"identity":{"timezone":"UTC"},"talent_overrides":overrides}),
        )
        .unwrap(),
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_journal_io::{
        AcceptedDailyResult, DailyUnitRecord, load_daily_unit_record, save_daily_unit_record,
    };
    use std::fs;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;
    use std::time::{Duration, Instant};

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let talent = dir.path().join("payload/talent");
        let apps = dir.path().join("payload/apps");
        fs::create_dir_all(&talent).unwrap();
        fs::create_dir_all(&apps).unwrap();
        fs::write(talent.join("schedule.md"), "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"output\":\"json\",\"schedule\":\"daily\",\"priority\":10,\"hook\":{\"post\":\"schedule\"}\n}\nExtract scheduled items.").unwrap();
        (dir, talent, apps)
    }
    fn source(journal: &Path, day: &str, text: &str) {
        let path = journal.join("chronicle").join(day).join("talents/flow.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn accept(journal: &Path, day: &str, talent: &Path, apps: &Path) -> DailyUnitRecord {
        let before = read_daily_coverage_with_roots(journal, day, talent, apps).unwrap();
        let unit = &before.units[0];
        let mut record = DailyUnitRecord::new(
            unit.identity.clone(),
            &unit.evidence_revision,
            &unit.contract_digest,
        );
        record.status = DailyUnitStatus::CommittedNoOutput;
        record.accepted = Some(AcceptedDailyResult {
            evidence_revision: unit.evidence_revision.clone(),
            contract_digest: unit.contract_digest.clone(),
            status: DailyUnitStatus::CommittedNoOutput,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(serde_json::json!({"response":"[]","output":"[]"})),
            receipts: Vec::new(),
            committed_at_ms: 1,
        });
        save_daily_unit_record(journal, &record).unwrap();
        record
    }
    /// The owed question, per state.  ⚠ Total classification is the point: if a
    /// state is ever added and not classified here, this reds rather than
    /// silently defaulting it to not-owed.
    #[test]
    fn owed_is_outstanding_or_unreadable_and_never_unverified_history() {
        for (state, owed, current) in [
            (CoverageState::Current, false, true),
            (CoverageState::CurrentDegraded, false, true),
            (CoverageState::Outstanding, true, false),
            (CoverageState::HistoricalUnverified, false, false),
            (CoverageState::Unreadable, true, false),
        ] {
            assert_eq!(state.is_owed(), owed, "is_owed({state:?})");
            assert_eq!(state.is_current(), current, "is_current({state:?})");
        }
    }

    #[test]
    fn current_requires_matching_evidence_and_contract_not_legacy_logs_or_terminal_failure() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        let day = "20260910";
        source(root, day, "# Flow\nMeeting at ten.");
        let health = root.join(format!("chronicle/{day}/health"));
        fs::create_dir_all(&health).unwrap();
        fs::write(
            health.join("old.jsonl"),
            "{\"event\":\"talent.complete\",\"mode\":\"daily\",\"name\":\"schedule\"}\n",
        )
        .unwrap();
        assert!(
            !read_daily_coverage_with_roots(root, day, &talent, &apps)
                .unwrap()
                .state
                .is_current()
        );
        let first = accept(root, day, &talent, &apps);
        assert_eq!(
            read_daily_coverage_with_roots(root, day, &talent, &apps)
                .unwrap()
                .state,
            CoverageState::Current
        );
        source(root, day, "# Flow\nMeeting moved to eleven.");
        assert_eq!(
            read_daily_coverage_with_roots(root, day, &talent, &apps)
                .unwrap()
                .state,
            CoverageState::Outstanding
        );
        let mut old_failure = first;
        old_failure.status = DailyUnitStatus::Failed;
        save_daily_unit_record(root, &old_failure).unwrap();
        assert!(
            !read_daily_coverage_with_roots(root, day, &talent, &apps)
                .unwrap()
                .state
                .is_current()
        );
        accept(root, day, &talent, &apps);
        assert!(
            read_daily_coverage_with_roots(root, day, &talent, &apps)
                .unwrap()
                .state
                .is_current()
        );
        fs::remove_file(root.join(format!("chronicle/{day}/talents/flow.md"))).unwrap();
        assert_eq!(
            read_daily_coverage_with_roots(root, day, &talent, &apps)
                .unwrap()
                .state,
            CoverageState::Outstanding
        );
    }
    #[test]
    fn matching_accepted_proof_cannot_hide_a_conflicting_or_pending_replacement() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        let day = "20260910";
        source(root, day, "# Flow\nMeeting at ten.");
        let mut record = accept(root, day, &talent, &apps);
        for status in [DailyUnitStatus::Conflicting, DailyUnitStatus::Unfinished] {
            record.status = status;
            save_daily_unit_record(root, &record).unwrap();
            assert_eq!(
                read_daily_coverage_with_roots(root, day, &talent, &apps)
                    .unwrap()
                    .state,
                CoverageState::Outstanding
            );
        }
        record.status = DailyUnitStatus::Capped;
        record.reason_code = Some("daily_owner_conflict".into());
        record.failure_count = 100;
        save_daily_unit_record(root, &record).unwrap();
        assert_eq!(
            read_daily_coverage_with_roots(root, day, &talent, &apps)
                .unwrap()
                .state,
            CoverageState::Outstanding
        );
    }

    #[test]
    fn explicit_old_day_registration_survives_restart_and_finds_lost_wake_changes() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        let day = "20260801";
        let now: DateTime<Utc> = "2026-09-14T12:00:00Z".parse().unwrap();
        source(root, day, "# Flow\nAn old meeting.");
        assert!(
            reconcile_days_with_roots(root, &[], now, &talent, &apps)
                .unwrap()
                .is_empty()
        );
        register_daily_day(root, day, now).unwrap();
        assert_eq!(
            read_daily_coverage_with_roots(root, day, &talent, &apps)
                .unwrap()
                .state,
            CoverageState::Outstanding
        );
        accept(root, day, &talent, &apps);
        assert!(
            reconcile_days_with_roots(root, &[], now, &talent, &apps)
                .unwrap()
                .is_empty()
        );
        source(
            root,
            day,
            "# Flow\nAn old meeting corrected without a wake.",
        );
        assert_eq!(
            reconcile_days_with_roots(root, &[], now, &talent, &apps).unwrap(),
            vec![day]
        );
    }

    #[test]
    fn adoption_persists_closed_calendar_boundary_and_reconciles_derived_only_changes() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            "{\"identity\":{\"timezone\":\"UTC\"}}",
        )
        .unwrap();
        for day in ["20260801", "20260907", "20260910", "20260914"] {
            source(root, day, "# Flow\nA meeting.");
        }
        let now: DateTime<Utc> = "2026-09-14T12:00:00Z".parse().unwrap();
        let pending = reconcile_days_with_roots(root, &[], now, &talent, &apps).unwrap();
        assert_eq!(pending, vec!["20260907", "20260910"]);
        assert_eq!(
            read_daily_coverage_with_roots(root, "20260910", &talent, &apps)
                .unwrap()
                .state,
            CoverageState::Outstanding
        );
        assert_eq!(
            read_daily_coverage_with_roots(root, "20260801", &talent, &apps)
                .unwrap()
                .state,
            CoverageState::HistoricalUnverified
        );
        accept(root, "20260907", &talent, &apps);
        accept(root, "20260910", &talent, &apps);
        assert!(
            reconcile_days_with_roots(root, &[], now, &talent, &apps)
                .unwrap()
                .is_empty()
        );
        source(root, "20260910", "# Flow\nA changed meeting.");
        assert_eq!(
            reconcile_days_with_roots(root, &[], now, &talent, &apps).unwrap(),
            vec!["20260910"]
        );
        let state: Value =
            serde_json::from_slice(&fs::read(root.join("health/daily-adoption.json")).unwrap())
                .unwrap();
        assert_eq!(state["first_closed_day"], "20260907");
        let pending =
            reconcile_days_with_roots(root, &["20260801".to_owned()], now, &talent, &apps).unwrap();
        assert!(pending.contains(&"20260801".to_owned()));
        assert!(!pending.contains(&"20260914".to_owned()));
    }

    #[test]
    fn review_uncommitted_started_receipt_is_outstanding_when_committed_or_capped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (talent, apps) = package_roots().unwrap();
        let overrides = daily_configs(root, &talent, &apps)
            .unwrap()
            .into_iter()
            .map(|config| {
                let key = match config.key.split_once(':') {
                    Some((app, name)) => format!("talent.{app}.{name}"),
                    None => format!("talent.system.{}", config.key),
                };
                (
                    key,
                    serde_json::json!({"disabled": config.key != "entities:entities_review"}),
                )
            })
            .collect::<serde_json::Map<String, Value>>();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            serde_json::to_vec(&serde_json::json!({
                "identity":{"timezone":"UTC"}, "talent_overrides": overrides
            }))
            .unwrap(),
        )
        .unwrap();
        fs::create_dir_all(root.join("facets/work")).unwrap();
        fs::write(root.join("facets/work/facet.json"), r#"{"name":"work"}"#).unwrap();
        let day = "20260910";
        let seg = root.join("chronicle").join(day).join("120000_60");
        fs::create_dir_all(seg.join("talents")).unwrap();
        fs::write(seg.join("talents/facets.json"), r#"[{"facet":"work"}]"#).unwrap();
        let coverage = read_daily_coverage_with_roots(root, day, &talent, &apps).unwrap();
        let unit = coverage
            .units
            .iter()
            .find(|unit| unit.identity.name == "entities:entities_review")
            .expect("review unit");
        let started = serde_json::json!({
            "kind": "owner_action",
            "action_id": "0:test",
            "token": "tok",
            "state": "started"
        });
        let mut committed = DailyUnitRecord::new(
            unit.identity.clone(),
            &unit.evidence_revision,
            &unit.contract_digest,
        );
        committed.status = DailyUnitStatus::CommittedNoOutput;
        committed.accepted = Some(AcceptedDailyResult {
            evidence_revision: unit.evidence_revision.clone(),
            contract_digest: unit.contract_digest.clone(),
            status: DailyUnitStatus::CommittedNoOutput,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(serde_json::json!({"response":"[]","output":"[]"})),
            receipts: Vec::new(),
            committed_at_ms: 1,
        });
        committed.receipts.push(started.clone());
        save_daily_unit_record(root, &committed).unwrap();
        let after = read_daily_coverage_with_roots(root, day, &talent, &apps).unwrap();
        let review = after
            .units
            .iter()
            .find(|unit| unit.identity.name == "entities:entities_review")
            .unwrap();
        assert_eq!(review.state, CoverageState::Outstanding);
        assert!(!review.state.is_current());

        let mut capped = committed;
        capped.status = DailyUnitStatus::Capped;
        capped.reason_code = Some("schema_invalid".into());
        capped.failure_count = 3;
        save_daily_unit_record(root, &capped).unwrap();
        let after = read_daily_coverage_with_roots(root, day, &talent, &apps).unwrap();
        let review = after
            .units
            .iter()
            .find(|unit| unit.identity.name == "entities:entities_review")
            .unwrap();
        assert_eq!(review.state, CoverageState::Outstanding);
        assert!(!review.state.is_current());
    }

    #[test]
    fn a_coverage_read_captures_the_day_once_for_all_of_its_units() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        let day = "20260910";
        let frontmatter = |kind: &str, extra: &str| {
            format!(
                "{{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"output\":\"md\",\"schedule\":\"daily\",\"priority\":20,{extra}\"hook\":{kind}\n}}\nBody."
            )
        };
        fs::write(
            talent.join("facet_newsletter.md"),
            frontmatter(
                "{\"pre\":\"facet_newsletter\",\"post\":\"facet_newsletter\"}",
                "\"multi_facet\":true,",
            ),
        )
        .unwrap();
        fs::write(
            talent.join("morning_briefing.md"),
            frontmatter("{\"pre\":\"morning_briefing\"}", ""),
        )
        .unwrap();
        source(root, day, "# Flow\nMeeting at ten.");
        // Two declared facets, both active on the day.
        for facet in ["work", "home"] {
            let dir = root.join("facets").join(facet);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("facet.json"), "{\"title\":\"Facet\"}").unwrap();
        }
        let segment = root.join(format!("chronicle/{day}/mic/090000_60/talents"));
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join("facets.json"),
            "[{\"facet\":\"work\"},{\"facet\":\"home\"}]",
        )
        .unwrap();

        let mut cache = DayProjectionCache::new(root, day);
        let mut shared =
            read_daily_coverage_with_cache(root, day, &talent, &apps, &mut cache).unwrap();
        // schedule + morning_briefing + one facet_newsletter per active facet, through both call sites.
        assert_eq!(shared.units.len(), 4, "{:?}", shared.units);
        assert_eq!(
            cache.captures(),
            1,
            "the day is captured once for all units"
        );
        assert_eq!(
            cache.requests() as usize,
            shared.units.len(),
            "no unit bypassed the cache"
        );

        // Same coverage as a plain read (which builds its own cache), apart from the read time.
        let mut plain = read_daily_coverage_with_roots(root, day, &talent, &apps).unwrap();
        shared.as_of_ms = 0;
        plain.as_of_ms = 0;
        assert_eq!(format!("{shared:?}"), format!("{plain:?}"));
    }

    // --- whole-day coverage reads are mutually exclusive within a process ------------

    /// Long enough that a read which is merely slow is never mistaken for a hung one.
    const GENEROUS: Duration = Duration::from_secs(60);

    type Read = Result<DailyCoverage, String>;

    fn read_on_a_thread(root: &Path, talent: &Path, apps: &Path) -> mpsc::Receiver<Read> {
        let (sender, receiver) = mpsc::channel();
        let (root, talent, apps) = (root.to_owned(), talent.to_owned(), apps.to_owned());
        thread::spawn(move || {
            let _ = sender.send(read_daily_coverage_with_roots(
                &root, "20260910", &talent, &apps,
            ));
        });
        receiver
    }

    /// The same coverage, apart from when it was read.
    fn assert_same_coverage(mut first: DailyCoverage, mut second: DailyCoverage) {
        first.as_of_ms = 0;
        second.as_of_ms = 0;
        assert_eq!(first, second);
    }

    #[test]
    fn acquiring_a_poisoned_lock_takes_it_over_instead_of_panicking() {
        let lock = Mutex::new(());
        thread::scope(|scope| {
            let poisoner = thread::Builder::new()
                .name("intentional-poison-of-a-private-lock".to_owned())
                .spawn_scoped(scope, || {
                    let _held = lock.lock().unwrap();
                    panic!("intentional: poisoning a private lock for the test");
                })
                .unwrap();
            assert!(poisoner.join().is_err());
        });
        assert!(lock.is_poisoned(), "the fixture must poison the lock");
        drop(acquire_coverage_read(&lock, "20260910"));
    }

    #[test]
    fn acquiring_waits_for_a_holder_and_then_takes_the_lock() {
        let lock = Mutex::new(());
        // Control: free, it is taken at once.
        drop(acquire_coverage_read(&lock, "20260910"));
        let held = lock.lock().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::scope(|scope| {
            scope.spawn(|| {
                drop(acquire_coverage_read(&lock, "20260910"));
                sender.send(()).unwrap();
            });
            let early = receiver.recv_timeout(Duration::from_millis(300));
            drop(held);
            assert!(
                matches!(early, Err(RecvTimeoutError::Timeout)),
                "took the lock while another held it: {early:?}"
            );
            receiver
                .recv_timeout(GENEROUS)
                .expect("taken once the holder let go");
        });
    }

    #[test]
    fn a_coverage_read_waits_while_another_holds_the_lock_and_then_matches_a_plain_read() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        source(root, "20260910", "# Flow\nMeeting at ten.");
        // Control: with nothing holding the lock the same read completes, and how long
        // that took sets how long the held case is given to (wrongly) finish.
        let started = Instant::now();
        let control = read_on_a_thread(root, &talent, &apps)
            .recv_timeout(GENEROUS)
            .expect("an uncontended read completes")
            .expect("the fixture reads");
        let took = started.elapsed();
        let patience = (took * 4).clamp(Duration::from_millis(300), Duration::from_secs(2));
        // The wait below must be long against the read itself, or a lockless build would
        // still be mid-read when it ends and the test would pass on the cap.
        assert!(
            took * 2 <= patience,
            "inconclusive: an uncontended read took {took:?}, too long to prove exclusion"
        );

        let held = COVERAGE_READ.lock().unwrap_or_else(PoisonError::into_inner);
        let blocked = read_on_a_thread(root, &talent, &apps);
        let early = blocked.recv_timeout(patience);
        // Released before any assertion, so a failure is a red test and not a hang.
        drop(held);
        assert!(
            matches!(early, Err(RecvTimeoutError::Timeout)),
            "a coverage read ran while another held the lock: {early:?}"
        );
        let released = blocked
            .recv_timeout(GENEROUS)
            .expect("the read completes once the lock is released")
            .expect("the fixture reads");
        assert_same_coverage(control, released);
    }

    #[test]
    fn a_read_of_the_maintenance_unit_inside_the_lock_does_not_deadlock() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        source(root, "20260910", "# Flow\nMeeting at ten.");
        // The maintenance unit is read with `read_unit_coverage` from inside the guarded
        // region; a second acquisition there would hang.
        fs::write(
            talent.join("daily_schedule.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"output\":\"json\",\"schedule\":\"daily\",\"priority\":5,\"hook\":{\"pre\":\"daily_schedule\",\"post\":\"daily_schedule\"}\n}\nMaintain the schedule.",
        )
        .unwrap();
        let first = read_on_a_thread(root, &talent, &apps)
            .recv_timeout(GENEROUS)
            .expect("the read does not deadlock on itself")
            .expect("the fixture reads");
        assert!(first.maintenance.is_some(), "the maintenance unit was read");
        let second = read_on_a_thread(root, &talent, &apps)
            .recv_timeout(GENEROUS)
            .expect("a second read on a new thread completes")
            .expect("the fixture reads");
        assert_same_coverage(first, second);
        // Two reads on one thread, one after the other.
        let one = read_daily_coverage_with_roots(root, "20260910", &talent, &apps).unwrap();
        let two = read_daily_coverage_with_roots(root, "20260910", &talent, &apps).unwrap();
        assert_same_coverage(one, two);
    }

    #[test]
    fn a_panic_while_holding_the_lock_does_not_break_later_coverage_reads() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        source(root, "20260910", "# Flow\nMeeting at ten.");
        let expected = read_daily_coverage_with_roots(root, "20260910", &talent, &apps).unwrap();
        let poisoner = thread::Builder::new()
            .name("intentional-poison-of-the-coverage-lock".to_owned())
            .spawn(|| {
                let _held = COVERAGE_READ.lock().unwrap_or_else(PoisonError::into_inner);
                panic!("intentional: poisoning the coverage lock for the test");
            })
            .unwrap();
        assert!(poisoner.join().is_err());
        assert!(
            COVERAGE_READ.is_poisoned(),
            "the fixture must poison the lock"
        );
        let after = read_on_a_thread(root, &talent, &apps)
            .recv_timeout(GENEROUS)
            .expect("a read after a panic completes")
            .expect("the fixture reads");
        COVERAGE_READ.clear_poison();
        assert_same_coverage(expected, after);
    }

    /// Days before the journal's real today; the coverage reader ages units
    /// against the wall clock, so these fixtures do too.
    fn closed_days(journal: &Path, count: i64) -> Vec<String> {
        let today =
            chrono::NaiveDate::parse_from_str(&local_day(journal, Utc::now()), "%Y%m%d").unwrap();
        (1..=count)
            .rev()
            .map(|back| {
                (today - chrono::Duration::days(back))
                    .format("%Y%m%d")
                    .to_string()
            })
            .collect()
    }

    fn utc_journal(journal: &Path, extra: Value) {
        let mut config = serde_json::json!({"identity":{"timezone":"UTC"}});
        for (key, value) in extra.as_object().into_iter().flatten() {
            config[key] = value.clone();
        }
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(journal.join("config/journal.json"), config.to_string()).unwrap();
    }

    fn unit_state(journal: &Path, day: &str, talent: &Path, apps: &Path) -> UnitCoverage {
        read_daily_coverage_with_roots(journal, day, talent, apps)
            .unwrap()
            .units
            .remove(0)
    }

    /// Catch-up used to resubmit a day its own run had just brought current,
    /// every drain pass, until the four-a-pass cursor came round (up to 13 runs
    /// a night for one day).  A run now settles its day on the way out.
    #[test]
    fn a_run_settles_its_own_current_day_so_catch_up_does_not_resubmit_it() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        utc_journal(root, serde_json::json!({}));
        let days = closed_days(root, 35);
        let now = Utc::now();
        for day in &days {
            source(root, day, "# Flow\nMeeting.");
            accept(root, day, &talent, &apps);
            register_daily_day(root, day, now).unwrap();
        }
        while !reconcile_days_with_roots(root, &[], now, &talent, &apps)
            .unwrap()
            .is_empty()
        {}
        let day = days.last().unwrap();
        // Control: registering alone leaves the day for the rotating cursor.
        register_daily_day(root, day, now).unwrap();
        let pending = reconcile_days_with_roots(root, &[], now, &talent, &apps).unwrap();
        assert!(
            pending.contains(day),
            "registration alone keeps {day} pending"
        );
        // A run that brings the day current settles it at once.
        assert!(settle_daily_day(root, day, &talent, &apps, now).unwrap());
        assert!(
            reconcile_days_with_roots(root, &[], now, &talent, &apps)
                .unwrap()
                .is_empty()
        );
        // Settling a day that is not pending changes nothing.
        assert!(!settle_daily_day(root, day, &talent, &apps, now).unwrap());
    }

    #[test]
    fn settling_keeps_a_day_that_still_owes_work() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        utc_journal(root, serde_json::json!({}));
        let day = closed_days(root, 1).remove(0);
        let now = Utc::now();
        source(root, &day, "# Flow\nMeeting.");
        accept(root, &day, &talent, &apps);
        register_daily_day(root, &day, now).unwrap();
        source(root, &day, "# Flow\nMeeting moved.");
        assert!(!settle_daily_day(root, &day, &talent, &apps, now).unwrap());
        assert_eq!(
            reconcile_days_with_roots(root, &[], now, &talent, &apps).unwrap(),
            vec![day]
        );
    }

    /// Operator approval, 2026-09-30: a contract change re-derives the last seven closed
    /// days; older closed days keep their output, marked as an earlier version.
    #[test]
    fn a_contract_change_reowes_only_the_last_seven_closed_days() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        utc_journal(
            root,
            serde_json::json!({"providers":{"active":{"provider":"local","model":"a"}}}),
        );
        let days = closed_days(root, 12);
        for day in &days {
            source(root, day, "# Flow\nMeeting.");
            accept(root, day, &talent, &apps);
        }
        for change in ["model", "facet"] {
            if change == "model" {
                utc_journal(
                    root,
                    serde_json::json!({"providers":{"active":{"provider":"local","model":"b"}}}),
                );
            } else {
                fs::create_dir_all(root.join("facets/work")).unwrap();
                fs::write(root.join("facets/work/facet.json"), r#"{"title":"Work"}"#).unwrap();
            }
            for (index, day) in days.iter().enumerate() {
                let unit = unit_state(root, day, &talent, &apps);
                if index < days.len() - 7 {
                    assert_eq!(
                        unit.state,
                        CoverageState::Current,
                        "{change}: {day} keeps its output"
                    );
                    assert_eq!(unit.earlier_version, Some(AcceptedReuse::EarlierContract));
                    assert_eq!(unit.owed_by, None);
                } else {
                    assert_eq!(
                        unit.state,
                        CoverageState::Outstanding,
                        "{change}: {day} is re-owed"
                    );
                    assert_eq!(unit.owed_by.as_deref(), Some("contract_changed"));
                }
            }
        }
        // An older day whose own evidence changes is still re-derived.
        source(root, &days[0], "# Flow\nA different meeting.");
        let unit = unit_state(root, &days[0], &talent, &apps);
        assert_eq!(unit.state, CoverageState::Outstanding);
        assert_eq!(unit.owed_by.as_deref(), Some("evidence_changed"));
    }

    /// Operator approval, 2026-09-30: once its morning has passed a briefing is a record
    /// of what the owner was told, and nothing regenerates it on its own.
    #[test]
    fn a_briefing_is_frozen_once_its_morning_has_passed() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        fs::remove_file(talent.join("schedule.md")).unwrap();
        fs::write(
            talent.join("morning_briefing.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"output\":\"json\",\"schedule\":\"daily\",\"priority\":50,\"hook\":{\"pre\":\"morning_briefing\"}\n}\nBrief the morning.",
        )
        .unwrap();
        utc_journal(root, serde_json::json!({}));
        let days = closed_days(root, 3); // D-3 and D-2 are past their morning; D-1 is today's.
        for day in &days {
            source(root, day, "# Flow\nMeeting.");
            accept(root, day, &talent, &apps);
        }
        fs::write(
            talent.join("morning_briefing.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"output\":\"json\",\"schedule\":\"daily\",\"priority\":50,\"hook\":{\"pre\":\"morning_briefing\"}\n}\nBrief the morning, differently.",
        )
        .unwrap();
        for day in &days {
            source(root, day, "# Flow\nMeeting, and new evidence.");
        }
        for day in &days[..2] {
            let unit = unit_state(root, day, &talent, &apps);
            assert_eq!(unit.state, CoverageState::Current, "{day} is frozen");
            assert_eq!(unit.earlier_version, Some(AcceptedReuse::FrozenBriefing));
        }
        let live = unit_state(root, &days[2], &talent, &apps);
        assert_eq!(
            live.state,
            CoverageState::Outstanding,
            "the briefing presented today is live"
        );

        // A later attempt that failed or was cut short keeps the accepted
        // result: a past briefing is not unfrozen by a failed re-run.
        let briefing = |day: &str| DailyUnitIdentity::new(day, "morning_briefing", None);
        for status in [
            DailyUnitStatus::Failed,
            DailyUnitStatus::Unfinished,
            DailyUnitStatus::Capped,
        ] {
            let mut record = load_daily_unit_record(root, &briefing(&days[0]))
                .unwrap()
                .unwrap();
            record.status = status;
            save_daily_unit_record(root, &record).unwrap();
            let unit = unit_state(root, &days[0], &talent, &apps);
            assert_eq!(
                unit.state,
                CoverageState::Current,
                "{status:?} leaves it frozen"
            );
        }
        // A frozen briefing whose file is gone is not regenerated.
        let mut record = load_daily_unit_record(root, &briefing(&days[0]))
            .unwrap()
            .unwrap();
        record.status = DailyUnitStatus::Committed;
        let accepted = record.accepted.as_mut().unwrap();
        accepted.status = DailyUnitStatus::Committed;
        accepted.receipts = vec![
            serde_json::json!({"kind": "owner_action", "state": "committed", "action_id": "a", "token": "t"}),
            serde_json::json!({"kind": "required_artifact", "path": "chronicle/gone.json", "sha256": "0".repeat(64)}),
        ];
        save_daily_unit_record(root, &record).unwrap();
        let unit = unit_state(root, &days[0], &talent, &apps);
        assert_eq!(
            unit.state,
            CoverageState::Current,
            "a frozen briefing stays frozen whatever became of its file"
        );
        // A newer attempt that committed an owner write resumes instead.
        let mut record = load_daily_unit_record(root, &briefing(&days[0]))
            .unwrap()
            .unwrap();
        record.status = DailyUnitStatus::Failed;
        record.receipts = vec![
            serde_json::json!({"kind": "owner_action", "state": "committed", "action_id": "b", "token": "u"}),
        ];
        save_daily_unit_record(root, &record).unwrap();
        let unit = unit_state(root, &days[0], &talent, &apps);
        assert_eq!(unit.state, CoverageState::Outstanding);
        // A conflict with the owner is never kept.
        let mut record = load_daily_unit_record(root, &briefing(&days[1]))
            .unwrap()
            .unwrap();
        record.status = DailyUnitStatus::Conflicting;
        save_daily_unit_record(root, &record).unwrap();
        let unit = unit_state(root, &days[1], &talent, &apps);
        assert_eq!(unit.state, CoverageState::Outstanding);
        assert_eq!(unit.owed_by.as_deref(), Some("conflict"));
        // A sentinel reset unfreezes the briefing and marks it Outstanding.
        let mut record = load_daily_unit_record(root, &briefing(&days[0]))
            .unwrap()
            .unwrap();
        record.evidence_revision = "owner-reprocess".into();
        record.contract_digest = "owner-reprocess".into();
        record.status = DailyUnitStatus::Unfinished;
        record.receipts.clear();
        save_daily_unit_record(root, &record).unwrap();
        let unit = unit_state(root, &days[0], &talent, &apps);
        assert_eq!(
            unit.state,
            CoverageState::Outstanding,
            "owner-reprocess sentinel must unfreeze accepted reuse"
        );
        assert_eq!(unit.earlier_version, None);
    }
}
