// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

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
    /// Unsupported evidence contracts stay unverified; an absent accepted
    /// record is outstanding regardless of reconciliation bookkeeping.
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
    /// `contract_changed`, `retry`, `unconfirmed_write` or `conflict`.  Lets a release state what it would
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
    local_now(journal, now).format("%Y%m%d").to_string()
}

/// The first owner-local day admitted by a registered input policy.
///
/// That day is the owner's calendar date at `now`, minus
/// the policy lookback. The chronicle is not scanned.
pub fn daily_input_since_day(
    now: DateTime<Utc>,
    zone: &solstone_core_journal_config::Tz,
    lookback_closed_days: i64,
) -> String {
    now.with_timezone(zone)
        .date_naive()
        .checked_sub_signed(chrono::Duration::days(lookback_closed_days))
        .expect("a calendar date minus the registered lookback")
        .format("%Y%m%d")
        .to_string()
}

/// Initialize registered input cutoffs before automatic or explicit coverage.
/// Reads remain pure; this coordinator delegates persistence to the config owner.
pub fn activate_daily_inputs(journal: &Path, now: DateTime<Utc>) -> Result<(), String> {
    activate_daily_inputs_checked(journal, now, &|| Ok(()))
}

pub fn activate_daily_inputs_checked(
    journal: &Path,
    now: DateTime<Utc>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    let zone = solstone_core_journal_config::owner_zone(journal);
    for input in solstone_core_indexer::daily_evidence::DAILY_INPUTS {
        let day = daily_input_since_day(now, &zone, input.lookback_closed_days);
        solstone_core_journal_config_write::insert_daily_input_since_checked(
            journal,
            input.id,
            input.version,
            &day,
            checkpoint,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The owner's wall-clock time at `now`, in the journal's owner zone.
pub fn local_now(journal: &Path, now: DateTime<Utc>) -> chrono::NaiveDateTime {
    now.with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .naive_local()
}

/// The hour of the owner's day at which a morning briefing's morning is over.
pub const BRIEFING_MORNING_ENDS_HOUR: u32 = 12;

/// Daily talents that no longer ship. Their old unit records and held retries
/// stay on disk, but nothing owes, shows or reruns them.
pub const RETIRED_DAILY_TALENTS: &[&str] = &["schedule"];

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
/// reconciliation), and it must never be held while taking the ledger or
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

/// Whether `record` answers for this unit at `now`, the owner's wall-clock
/// time.  `None` means it is owed.
///
/// `Some(None)` is an exact match on evidence and contract; `Some(Some(_))` is
/// an accepted result kept on purpose.  Either stands whatever became of its
/// files.  ⛔ Every reader that decides reuse goes through this --
/// the coverage reader and the run's own skip must never disagree, or a day
/// reads current while its run regenerates it.
pub fn accepted_reuse(
    record: &solstone_core_journal_io::DailyUnitRecord,
    evidence: &DailyEvidence,
    now: chrono::NaiveDateTime,
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
    let today = now.date();
    // The briefing for day D is presented on the morning of D+1, and its
    // morning is over at noon.
    if record.identity.name == "morning_briefing"
        && (day + chrono::Duration::days(1))
            .and_hms_opt(BRIEFING_MORNING_ENDS_HOUR, 0, 0)
            .is_some_and(|noon| now >= noon)
    {
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
    let now = local_now(journal, Utc::now());
    let record = observe_daily_unit_record(journal, &identity).map_err(|e| e.to_string())?;
    let mut reason = None;
    let mut earlier_version = None;
    let state = match &record {
        None => CoverageState::Outstanding,
        Some(record) => {
            if record.evidence_revision == e && record.contract_digest == contract {
                reason = record.reason_code.clone();
            }
            if record.status == DailyUnitStatus::Conflicting
                || (identity.name == "entities:entities_review"
                    && record.has_uncommitted_started_receipt())
            {
                CoverageState::Outstanding
            } else if let Some(reuse) = accepted_reuse(record, &evidence, now) {
                // An accepted result stands whatever became of its files, so
                // it is never work to redo. The run's own skip reads the same,
                // so the two never disagree.
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
            Some(owed_cause(record.as_ref(), &evidence).to_owned())
        } else {
            None
        },
    })
}

/// Why an owed unit is owed, for `journal reprocess --owed`.
fn owed_cause(
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
    if accepted.evidence_revision == evidence.revision
        && accepted.contract_digest == evidence.contract
    {
        return "retry";
    }
    if evidence.revision_under(&accepted.contract_digest) == accepted.evidence_revision {
        return "contract_changed";
    }
    "evidence_changed"
}

pub fn daily_failure_capped(reason: &str, count: u32) -> bool {
    let cap = match reason {
        "model_not_found" | "provider_request_rejected" => 1,
        "schema_invalid" => 3,
        "agent_stuck"
        | "context_window_exceeded"
        | "max_turns_exhausted"
        | "no_output"
        // No longer emitted; day-unit records already on disk from earlier
        // versions can carry it and must stay capped so coverage and dispatch agree.
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
    matches!(
        record.owner_conflict_kind.as_deref(),
        Some("merge_proposal_preparation" | "merge_proposals_changed")
    )
}

/// Entity review conflicts found before the conflicting action writes. Each
/// is raised while preparing the plan, or by a publish check that runs before
/// the action records that it started, so the journal holds every earlier
/// action whole and nothing of this one. A new kind joins only once the same
/// holds for it.
const PRE_WRITE_REVIEW_CONFLICTS: &[&str] = &[
    "alias_claimed",
    "alias_target_detached",
    "promotion_owner_state",
    "identity_moved",
    "identity_changed",
    "identity_disappeared",
    "identity_blocked",
    "promoted_entity_blocked",
    "identity_map_group_lost",
    "name_matches_multiple",
    "no_usable_entity_id",
    "identity_unreadable",
    "relationship_occupied",
    "relationship_changed",
    "merge_proposal_preparation",
    "merge_proposals_changed",
    "output_artifact_changed",
    "owning_facet_changed",
    "artifact_before_changed",
    "identity_merged",
    "identity_deleted",
    "principal_grant_refused",
];

/// An entity review stopped on a conflict that a fresh preparation can clear:
/// every change it started also committed, and the conflict was found before
/// its action wrote. The owner's retry request replaces the attempt; committed
/// changes stay in the journal, and the new preparation reads them as the
/// journal's current state.
pub fn review_conflict_reprepares(record: &solstone_core_journal_io::DailyUnitRecord) -> bool {
    record.identity.name == "entities:entities_review"
        && record.status == DailyUnitStatus::Conflicting
        && !record.has_uncommitted_started_receipt()
        && record
            .owner_conflict_kind
            .as_deref()
            .is_some_and(|kind| PRE_WRITE_REVIEW_CONFLICTS.contains(&kind))
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
    use std::sync::mpsc;
    #[cfg(feature = "full-tests")]
    use std::sync::mpsc::RecvTimeoutError;
    use std::thread;
    use std::time::Duration;
    #[cfg(feature = "full-tests")]
    use std::time::Instant;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let talent = dir.path().join("payload/talent");
        let apps = dir.path().join("payload/apps");
        fs::create_dir_all(&talent).unwrap();
        fs::create_dir_all(&apps).unwrap();
        fs::create_dir_all(apps.join("entities/talent")).unwrap();
        fs::write(
            suggest_path(&apps),
            suggest_talent("Propose durable facts."),
        )
        .unwrap();
        // The daily unit is facet-scoped: one declared facet, active on every
        // day a fixture gives material (`source`).
        let work = dir.path().join("facets/work");
        fs::create_dir_all(&work).unwrap();
        fs::write(work.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        (dir, talent, apps)
    }
    const SUGGEST: &str = "entities:entity_suggest";
    fn suggest_path(apps: &Path) -> PathBuf {
        apps.join("entities/talent/entity_suggest.md")
    }
    fn suggest_talent(body: &str) -> String {
        format!(
            "{{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"output\":\"json\",\"schedule\":\"daily\",\"priority\":57,\"multi_facet\":true,\"hook\":{{\"pre\":\"{SUGGEST}\",\"post\":\"{SUGGEST}\"}}\n}}\n{body}"
        )
    }
    fn suggest(day: &str) -> DailyUnitIdentity {
        DailyUnitIdentity::new(day, SUGGEST, Some("work".to_owned()))
    }
    fn source(journal: &Path, day: &str, text: &str) {
        let path = journal.join("chronicle").join(day).join("talents/flow.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        let segment = journal
            .join("chronicle")
            .join(day)
            .join("120000_60/talents");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("facets.json"), r#"[{"facet":"work"}]"#).unwrap();
    }
    fn accept(journal: &Path, day: &str, talent: &Path, apps: &Path) -> DailyUnitRecord {
        let before = read_daily_coverage_with_roots(journal, day, talent, apps).unwrap();
        accept_unit(journal, &before.units[0])
    }
    fn accept_named(
        journal: &Path,
        day: &str,
        name: &str,
        talent: &Path,
        apps: &Path,
    ) -> DailyUnitRecord {
        let before = read_daily_coverage_with_roots(journal, day, talent, apps).unwrap();
        let unit = before
            .units
            .iter()
            .find(|unit| unit.identity.name == name)
            .unwrap_or_else(|| panic!("{name} missing from {day}"));
        accept_unit(journal, unit)
    }
    fn accept_unit(journal: &Path, unit: &UnitCoverage) -> DailyUnitRecord {
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
            "{\"event\":\"talent.complete\",\"mode\":\"daily\",\"name\":\"entities:entity_suggest\",\"facet\":\"work\"}\n",
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
    /// A made output the owner deleted or edited is theirs.  The unit stays
    /// current, so catch-up neither remakes it nor resubmits its day forever.
    #[test]
    fn a_deleted_or_edited_output_is_the_owners_and_is_not_owed() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        let day = "20260910";
        source(root, day, "# Flow\nMeeting at ten.");
        let mut record = accept(root, day, &talent, &apps);
        let accepted = record.accepted.as_mut().unwrap();
        accepted.status = DailyUnitStatus::Committed;
        accepted.receipts = vec![
            serde_json::json!({"kind":"owner_action","state":"committed","action_id":"a","token":"t"}),
            // Its file is gone, or no longer the bytes that were made.
            serde_json::json!({"kind":"required_artifact","path":"out/made.md","sha256":"0".repeat(64)}),
        ];
        record.status = DailyUnitStatus::Committed;
        save_daily_unit_record(root, &record).unwrap();
        assert!(!solstone_core_journal_io::accepted_daily_artifacts_valid(root, &record).unwrap());
        let coverage = read_daily_coverage_with_roots(root, day, &talent, &apps).unwrap();
        assert_eq!(coverage.state, CoverageState::Current);
        assert_eq!(coverage.units[0].owed_by, None);
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
        // The suggestion unit for each active facet and the briefing, through both call sites.
        assert_eq!(shared.units.len(), 3, "{:?}", shared.units);
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
    //
    // The two tests that prove exclusion by waiting out a timeout on the wall clock run
    // under `full-tests`: how long a read takes depends on the host, so they cannot be
    // routine. The rest only bound a hang and stay in the routine set.

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

    #[cfg(feature = "full-tests")]
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

    #[cfg(feature = "full-tests")]
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
                // A newly declared facet, not active on any of these days.
                fs::create_dir_all(root.join("facets/home")).unwrap();
                fs::write(root.join("facets/home/facet.json"), r#"{"title":"Home"}"#).unwrap();
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
        let before: Vec<_> = days
            .iter()
            .map(|day| unit_state(root, day, &talent, &apps))
            .collect();
        let today = local_day(root, Utc::now());
        solstone_core_journal_config_write::insert_daily_input_since(
            root,
            "location",
            "location-words-1",
            &today,
        )
        .unwrap();
        for (day, previous) in days.iter().zip(before) {
            let unit = unit_state(root, day, &talent, &apps);
            assert_eq!(unit.state, previous.state, "{day}");
            assert_eq!(unit.earlier_version, previous.earlier_version, "{day}");
            assert_eq!(unit.owed_by, previous.owed_by, "{day}");
        }
    }

    /// 2.0.42's first start wrote the derived `location.md` into every past day holding
    /// phone location, and each such day's outputs read as owed by changed evidence.
    /// No daily packet reads the document, so neither its backfill nor a later rewrite
    /// re-owes any day; a day's own material still does. Activating shared location
    /// words re-owes only the closed days on or after the cutoff.
    #[test]
    fn a_location_backfill_reowes_no_day_and_the_days_own_material_still_does() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        utc_journal(root, serde_json::json!({}));
        let days = closed_days(root, 12);
        for day in &days {
            source(root, day, "# Flow\nMeeting.");
            accept(root, day, &talent, &apps);
        }
        let location = |day: &str, text: &str| {
            let chronicle = root.join("chronicle").join(day);
            fs::write(chronicle.join("location.md"), text).unwrap();
            fs::create_dir_all(chronicle.join("health")).unwrap();
            fs::write(chronicle.join("health/location.json"), "{\"version\":1}").unwrap();
        };
        // The upgrade's backfill, into days both older than and inside the window.
        for day in &days {
            location(
                day,
                "# location\n\n- readings near Golden, 14:00 to 14:09.\n",
            );
        }
        for day in &days {
            let unit = unit_state(root, day, &talent, &apps);
            assert_eq!(unit.state, CoverageState::Current, "{day} keeps its output");
            assert_eq!(unit.earlier_version, None, "{day} is an exact match");
            assert_eq!(unit.owed_by, None);
        }
        // Late phone location for an old day: its segment arrives and the document
        // is rebuilt. Neither is anything a daily packet reads.
        let segment = root
            .join("chronicle")
            .join(&days[0])
            .join("phone/140000_300");
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join("location.jsonl"),
            "{\"type\":\"reading\",\"latitude\":39.75,\"longitude\":-105.22}\n",
        )
        .unwrap();
        location(
            &days[0],
            "# location\n\n- readings near Golden, 14:00 to 14:30.\n",
        );
        assert_eq!(
            unit_state(root, &days[0], &talent, &apps).state,
            CoverageState::Current
        );
        let today =
            chrono::NaiveDate::parse_from_str(&local_day(root, Utc::now()), "%Y%m%d").unwrap();
        let cutoff = (today - chrono::Duration::days(CONTRACT_REOWE_CLOSED_DAYS))
            .format("%Y%m%d")
            .to_string();
        solstone_core_journal_config_write::insert_daily_input_since(
            root,
            "location",
            "location-words-1",
            &cutoff,
        )
        .unwrap();
        for (index, day) in days.iter().enumerate() {
            let unit = unit_state(root, day, &talent, &apps);
            if index < days.len() - CONTRACT_REOWE_CLOSED_DAYS as usize {
                assert_eq!(unit.state, CoverageState::Current, "{day}");
                assert_eq!(unit.earlier_version, None, "{day}");
                assert_eq!(unit.owed_by, None, "{day}");
            } else {
                assert_eq!(unit.state, CoverageState::Outstanding, "{day}");
                assert_eq!(unit.owed_by.as_deref(), Some("contract_changed"), "{day}");
            }
        }
        // Late material the day's outputs are made from re-owes it, old or recent.
        for day in [&days[0], &days[days.len() - 1]] {
            source(root, day, "# Flow\nA different meeting.");
            let unit = unit_state(root, day, &talent, &apps);
            assert_eq!(unit.state, CoverageState::Outstanding, "{day}");
            assert_eq!(unit.owed_by.as_deref(), Some("evidence_changed"), "{day}");
        }
    }

    /// Operator approval, 2026-09-30: once its morning has passed a briefing is a record
    /// of what the owner was told, and nothing regenerates it on its own.
    #[test]
    fn a_briefing_is_frozen_once_its_morning_has_passed() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        fs::remove_file(suggest_path(&apps)).unwrap();
        fs::write(
            talent.join("morning_briefing.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"output\":\"json\",\"schedule\":\"daily\",\"priority\":50,\"hook\":{\"pre\":\"morning_briefing\"}\n}\nBrief the morning.",
        )
        .unwrap();
        utc_journal(root, serde_json::json!({}));
        // D-3 and D-2 are past their morning whatever the hour; D-1's freezes
        // at noon, which the reuse predicate's own test pins.
        let days = closed_days(root, 3);
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
        let today =
            chrono::NaiveDate::parse_from_str(&local_day(root, Utc::now()), "%Y%m%d").unwrap();
        let cutoff = (today - chrono::Duration::days(CONTRACT_REOWE_CLOSED_DAYS))
            .format("%Y%m%d")
            .to_string();
        solstone_core_journal_config_write::insert_daily_input_since(
            root,
            "location",
            "location-words-1",
            &cutoff,
        )
        .unwrap();
        for day in &days[..2] {
            let unit = unit_state(root, day, &talent, &apps);
            assert_eq!(unit.state, CoverageState::Current, "{day} stays frozen");
            assert_eq!(unit.earlier_version, Some(AcceptedReuse::FrozenBriefing));
        }

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

    #[test]
    fn daily_input_since_day_is_local_midnight_minus_seven() {
        let zone: solstone_core_journal_config::Tz = "America/Denver".parse().unwrap();
        let before_midnight =
            chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 10, 12, 5, 59, 59).unwrap();
        let at_midnight =
            chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 10, 12, 6, 0, 0).unwrap();
        assert_eq!(daily_input_since_day(before_midnight, &zone, 7), "20261004");
        assert_eq!(daily_input_since_day(at_midnight, &zone, 7), "20261005");
    }

    #[test]
    fn read_daily_coverage_on_a_journal_with_no_config_file_creates_none() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        source(root, "20260910", "# Flow\nMeeting.");
        let coverage = read_daily_coverage_with_roots(root, "20260910", &talent, &apps).unwrap();
        assert!(!coverage.units.is_empty());
        assert!(!root.join("config/journal.json").exists());
    }

    #[test]
    fn activation_reowes_exactly_seven_closed_days() {
        let (dir, talent, apps) = fixture();
        let root = dir.path();
        utc_journal(root, serde_json::json!({}));
        let zone: solstone_core_journal_config::Tz = "UTC".parse().unwrap();
        let now = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 10, 12, 15, 0, 0).unwrap();
        let today = now.date_naive();
        let wall = now.naive_utc();
        let days: Vec<String> = (1..=120)
            .rev()
            .map(|back| {
                (today - chrono::Duration::days(back))
                    .format("%Y%m%d")
                    .to_string()
            })
            .collect();
        for day in &days {
            source(root, day, "# Flow\nMeeting.");
            accept_named(root, day, SUGGEST, &talent, &apps);
        }
        fs::write(
            talent.join("morning_briefing.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"output\":\"json\",\"schedule\":\"daily\",\"priority\":50,\"hook\":{\"pre\":\"morning_briefing\"}\n}\nBrief the morning.",
        )
        .unwrap();
        let briefing_day = (today - chrono::Duration::days(3))
            .format("%Y%m%d")
            .to_string();
        accept_named(root, &briefing_day, "morning_briefing", &talent, &apps);
        let cutoff = daily_input_since_day(now, &zone, 7);
        assert_eq!(
            cutoff,
            (today - chrono::Duration::days(CONTRACT_REOWE_CLOSED_DAYS))
                .format("%Y%m%d")
                .to_string()
        );
        assert_eq!(
            solstone_core_journal_config_write::insert_daily_input_since(
                root,
                "location",
                "location-words-1",
                &cutoff
            )
            .unwrap(),
            cutoff
        );
        let configs = daily_configs(root, &talent, &apps).unwrap();
        let suggestion = configs.iter().find(|config| config.key == SUGGEST).unwrap();
        let briefing = configs
            .iter()
            .find(|config| config.key == "morning_briefing")
            .unwrap();
        let evidence = |config: &solstone_core_talent_config::TalentConfig, day: &str| {
            let facet = (config.key == SUGGEST).then_some("work");
            solstone_core_indexer::daily_evidence::compute_daily_evidence(
                root,
                day,
                &config.key,
                &config.metadata,
                &config.body,
                facet,
                None,
            )
            .unwrap()
        };
        let mut reowed = Vec::new();
        for day in &days {
            let record = load_daily_unit_record(root, &suggest(day))
                .unwrap()
                .unwrap();
            let current = evidence(suggestion, day);
            let reuse = accepted_reuse(&record, &current, wall);
            let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d").unwrap();
            if date >= chrono::NaiveDate::parse_from_str(&cutoff, "%Y%m%d").unwrap() {
                assert_eq!(reuse, None, "{day}");
                reowed.push(day.clone());
            } else {
                assert_eq!(reuse, Some(None), "{day}");
                assert_eq!(record.evidence_revision, current.revision, "{day}");
                assert_eq!(record.contract_digest, current.contract, "{day}");
                assert_ne!(reuse, Some(Some(AcceptedReuse::EarlierContract)));
            }
        }
        assert_eq!(reowed.len(), CONTRACT_REOWE_CLOSED_DAYS as usize);
        let briefing_record = load_daily_unit_record(
            root,
            &DailyUnitIdentity::new(&briefing_day, "morning_briefing", None),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            accepted_reuse(&briefing_record, &evidence(briefing, &briefing_day), wall),
            Some(Some(AcceptedReuse::FrozenBriefing))
        );
        let later = now + chrono::Duration::days(3);
        let later_cutoff = daily_input_since_day(later, &zone, 7);
        assert_ne!(later_cutoff, cutoff);
        assert_eq!(
            solstone_core_journal_config_write::insert_daily_input_since(
                root,
                "location",
                "location-words-1",
                &later_cutoff
            )
            .unwrap(),
            cutoff
        );
        let stored = solstone_core_journal_config::read_journal_config(root)
            .unwrap()
            .config
            .unwrap();
        assert_eq!(
            stored["daily_inputs"]["location"]["location-words-1"].as_str(),
            Some(cutoff.as_str())
        );
        source(root, &days[0], "# Flow\nA different meeting.");
        let oldest = load_daily_unit_record(root, &suggest(&days[0]))
            .unwrap()
            .unwrap();
        assert_eq!(
            accepted_reuse(&oldest, &evidence(suggestion, &days[0]), wall),
            None
        );
        let recent = days.last().unwrap();
        source(root, recent, "# Flow\nA different meeting.");
        let recent_record = load_daily_unit_record(root, &suggest(recent))
            .unwrap()
            .unwrap();
        assert_eq!(
            accepted_reuse(&recent_record, &evidence(suggestion, recent), wall),
            None
        );
        let open = today.format("%Y%m%d").to_string();
        source(root, &open, "# Flow\nMeeting.");
        accept_named(root, &open, SUGGEST, &talent, &apps);
        fs::write(
            suggest_path(&apps),
            suggest_talent("Propose durable facts, differently."),
        )
        .unwrap();
        let rewritten = daily_configs(root, &talent, &apps).unwrap();
        let suggestion = rewritten
            .iter()
            .find(|config| config.key == SUGGEST)
            .unwrap();
        for day in days.iter().skip(1).chain(std::iter::once(&open)) {
            let record = load_daily_unit_record(root, &suggest(day))
                .unwrap()
                .unwrap();
            let current = evidence(suggestion, day);
            let reuse = accepted_reuse(&record, &current, wall);
            let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d").unwrap();
            if date < today - chrono::Duration::days(CONTRACT_REOWE_CLOSED_DAYS) {
                assert_eq!(reuse, Some(Some(AcceptedReuse::EarlierContract)), "{day}");
            } else {
                assert_eq!(reuse, None, "{day}");
            }
        }
    }

    #[test]
    fn daily_failure_capped_confidential_access_ended() {
        for count in 0..10 {
            assert!(!daily_failure_capped("confidential_access_ended", count));
        }
    }
}
