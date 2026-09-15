// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Evidence-bound daily coverage and bounded adoption/reconciliation bookkeeping.
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{DailyUnitIdentity, DailyUnitStatus, load_daily_unit_record};
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

pub fn local_day(journal: &Path, now: DateTime<Utc>) -> Result<String, String> {
    let path = journal.join("config/journal.json");
    let config: Value = match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => return Err(e.to_string()),
    };
    let zone = config
        .pointer("/identity/timezone")
        .and_then(Value::as_str)
        .unwrap_or("");
    if zone.is_empty() {
        return Ok(now.with_timezone(&Local).format("%Y%m%d").to_string());
    }
    let zone: chrono_tz::Tz = zone
        .parse()
        .map_err(|_| format!("invalid journal timezone {zone}"))?;
    Ok(now.with_timezone(&zone).format("%Y%m%d").to_string())
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
    let configs = daily_configs(journal, talent, apps)?;
    let facets =
        solstone_core_facets::list_declared_facet_names(journal).map_err(|e| e.to_string())?;
    let active = crate::activity_state::active_facets_checked(journal, day)?;
    let mut units = Vec::new();
    let mut maintenance = None;
    for config in configs {
        // Global maintenance is reported independently of historical evidence coverage.
        if config.key == "daily_schedule" {
            let today = local_day(journal, Utc::now())?;
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
                units.push(read_unit_coverage(journal, day, &config, Some(facet))?);
            }
        } else {
            units.push(read_unit_coverage(journal, day, &config, None)?);
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
    let identity = DailyUnitIdentity::new(day, &config.key, facet.map(str::to_owned));
    let revision = solstone_core_indexer::daily_evidence::compute_daily_evidence_revision(
        journal,
        day,
        &config.key,
        &config.metadata,
        &config.body,
        facet,
        None,
    );
    let (e, contract) = match revision {
        Ok(revision) => revision,
        Err(error) if error.starts_with("unsupported") => {
            return Ok(UnitCoverage {
                identity,
                evidence_revision: String::new(),
                contract_digest: String::new(),
                state: CoverageState::HistoricalUnverified,
                reason_code: Some(error),
            });
        }
        Err(error) => return Err(error),
    };
    let record = load_daily_unit_record(journal, &identity).map_err(|e| e.to_string())?;
    let mut reason = None;
    let state = match record {
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
            if record.status == DailyUnitStatus::Conflicting {
                CoverageState::Outstanding
            } else if record.status.is_terminal_success()
                && record.is_reusable_for(&e, &contract)
                && solstone_core_journal_io::accepted_daily_artifacts_valid(journal, &record)
                    .map_err(|e| e.to_string())?
            {
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
    })
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
    let state: Adoption = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(state) => state,
            Err(_) => return Ok(false),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
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
    let today = local_day(journal, now)?;
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
    let state: Adoption = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| fresh()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Adoption {
            version: 1,
            first_closed_day: (date - chrono::Duration::days(7))
                .format("%Y%m%d")
                .to_string(),
            ..Adoption::default()
        },
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
    let today = local_day(journal, now)?;
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
        let still_pending = match read_daily_coverage_with_roots(journal, &day, talent, apps) {
            Ok(coverage) => {
                let retry_due = coverage.units.iter().any(|unit| {
                    unit.state == CoverageState::CurrentDegraded
                        && unit
                            .reason_code
                            .as_deref()
                            .is_some_and(environmental_failure)
                        && load_daily_unit_record(journal, &unit.identity)
                            .ok()
                            .flatten()
                            .is_some_and(|record| {
                                record.environmental_retry_day.as_deref() != Some(today.as_str())
                            })
                });
                coverage.state.is_owed() || retry_due || raw_marker_dirty(journal, &day)?
            }
            Err(_) => true,
        };
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
    use solstone_core_journal_io::{AcceptedDailyResult, DailyUnitRecord, save_daily_unit_record};
    use std::fs;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let talent = dir.path().join("payload/talent");
        let apps = dir.path().join("payload/apps");
        fs::create_dir_all(&talent).unwrap();
        fs::create_dir_all(&apps).unwrap();
        fs::write(talent.join("schedule.md"), "{\n\"type\":\"generate\",\"output\":\"json\",\"schedule\":\"daily\",\"priority\":10,\"hook\":{\"post\":\"schedule\"}\n}\nExtract scheduled items.").unwrap();
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
}
