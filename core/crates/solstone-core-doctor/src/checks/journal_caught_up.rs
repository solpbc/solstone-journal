// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{BacklogDays, Check, RunnerResult, Status, make_result},
};
const CANT_TELL: &str = "re-run solstone journal doctor; check the health logs if it persists";

fn shell_argument(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c))
    {
        return value.to_owned();
    }
    #[cfg(windows)]
    let escaped = value.replace('\'', "''");
    #[cfg(not(windows))]
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

fn unfinished_suffix(days: &[solstone_core_system_health::BacklogDay]) -> String {
    let unfinished = solstone_core_system_health::aggregate_unfinished_from_days(days);
    if unfinished.activities > 0 {
        let act_str = if unfinished.activities == 1 {
            "1 activity".to_owned()
        } else {
            format!("{} activities", unfinished.activities)
        };
        let day_str = if unfinished.day_count == 1 {
            "1 completed day".to_owned()
        } else {
            format!("{} completed days", unfinished.day_count)
        };
        format!("; {act_str} on {day_str} couldn't finish processing")
    } else {
        String::new()
    }
}

/// Every result read from the backlog carries the days behind its counts, so
/// a reader has the set rather than deriving it.
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let mut days = None;
    let mut result = evaluate(context, check, &mut days)?;
    result.backlog_days = days;
    let inventory = solstone_core_system::work_inventory::read(&context.journal_path);
    let (summary, complete) = match inventory {
        Ok(Some(report)) => {
            let fresh = report.current_at(context.now.as_millisecond());
            let owed = report.bands.iter().any(|band| {
                band.segments_owed
                    + band.daily_units_owed
                    + band.activities_owed
                    + band.owner_requests
                    > 0
            });
            let bands = report.bands.iter().enumerate().map(|(i, band)| format!(
                "band {}: {} recordings ready / {} left, {} daily outputs ready / {} left, {} activities left, {} owner requests, {} waiting for permission, {} waiting for prerequisites",
                i + 1, band.segments_complete, band.segments_owed, band.daily_units_complete,
                band.daily_units_owed, band.activities_owed, band.owner_requests, band.permission_held, band.prerequisite_held,
            )).collect::<Vec<_>>().join("; ");
            (
                format!(
                    "inspected {} of {} days, {} through {}, as of {}; {}; {}{}; set aside: {}",
                    report.days_inspected,
                    report.days_total,
                    report.oldest_day.as_deref().unwrap_or("none"),
                    report.newest_day.as_deref().unwrap_or("none"),
                    report.as_of_ms,
                    bands,
                    if report.complete {
                        "whole-journal inspection"
                    } else {
                        "incomplete inspection"
                    },
                    if fresh { "" } else { "; last known progress" },
                    report
                        .holds
                        .iter()
                        .map(|hold| format!(
                            "{}: {}{}",
                            hold.day,
                            hold.reason,
                            hold.retry_unit
                                .as_ref()
                                .map(|id| format!(" (retry {id})"))
                                .unwrap_or_default()
                        ))
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
                fresh
                    && report.complete
                    && report.unreadable.is_empty()
                    && report.holds.is_empty()
                    && !owed,
            )
        }
        Ok(None) => (
            "whole-journal inspection has not reported yet".into(),
            false,
        ),
        Err(error) => (
            format!("whole-journal inspection unavailable: {error}"),
            false,
        ),
    };
    if !complete {
        if result.status == Status::Ok {
            result.status = Status::Warn;
        }
        if result.detail.starts_with("caught up") {
            result.detail = result
                .detail
                .replacen("caught up", "recent work checked", 1);
        }
        if result.fix.is_none() {
            result.fix = Some("open health to see waiting work and its next step".into());
        }
    }
    result.detail.push_str(&format!("; {summary}"));
    Ok(result)
}

fn backlog_days(view: &solstone_core_system_health::BacklogView) -> BacklogDays {
    let mut days = BacklogDays::default();
    for day in &view.days {
        let list = match day.state.as_str() {
            solstone_core_system_health::BACKLOG_STATE_PENDING => &mut days.pending,
            solstone_core_system_health::BACKLOG_STATE_STUCK => &mut days.stuck,
            solstone_core_system_health::BACKLOG_STATE_UNKNOWN => &mut days.unknown,
            _ => continue,
        };
        list.push(day.day.clone());
    }
    for list in [&mut days.pending, &mut days.stuck, &mut days.unknown] {
        list.sort();
    }
    days
}

fn evaluate(context: &CheckContext, check: Check, days: &mut Option<BacklogDays>) -> RunnerResult {
    let source = solstone_core_system_health::FilesystemHealthLogSource::new(&context.journal_path);
    let segments = solstone_core_system_health::FilesystemSegmentSource;
    let view = match solstone_core_system_health::read_backlog_view(
        &source,
        &segments,
        &context.journal_path,
        solstone_core_system_health::BACKLOG_DEFAULT_WINDOW,
        context.now,
    ) {
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Warn,
                format!("couldn't fully determine; backlog read failed: {error}"),
                Some(CANT_TELL),
            ));
        }
        Ok(view) => view,
    };
    *days = Some(backlog_days(&view));

    if !view.errors.is_empty()
        || view
            .days
            .iter()
            .any(|day| day.state == solstone_core_system_health::BACKLOG_STATE_UNKNOWN)
    {
        let unknown = view
            .days
            .iter()
            .filter(|day| day.state == solstone_core_system_health::BACKLOG_STATE_UNKNOWN)
            .count();
        let suffix = unfinished_suffix(&view.days);
        return Ok(make_result(
            check,
            Status::Warn,
            format!("couldn't fully determine; {unknown} day(s) unknown{suffix}"),
            Some(CANT_TELL),
        ));
    }

    let today = solstone_core_system::daily_coverage::local_day(&context.journal_path, context.now);
    let (talent_root, apps_root) = match context.payload_root.as_ref() {
        Some(payload) => (
            payload.join("solstone/talent"),
            payload.join("solstone/apps"),
        ),
        None => match solstone_core_system::daily_coverage::package_roots() {
            Ok(roots) => roots,
            Err(err) => {
                return Ok(make_result(
                    check,
                    Status::Warn,
                    format!(
                        "couldn't fully determine; couldn't locate the daily processing definitions: {err}"
                    ),
                    Some(CANT_TELL),
                ));
            }
        },
    };
    let configs = match solstone_core_system::daily_coverage::daily_configs(
        &context.journal_path,
        &talent_root,
        &apps_root,
    ) {
        Ok(c) => c,
        Err(err) => {
            return Ok(make_result(
                check,
                Status::Warn,
                format!("couldn't fully determine; failed to read daily configs: {err}"),
                Some(CANT_TELL),
            ));
        }
    };
    let configs_map: std::collections::BTreeMap<String, _> =
        configs.iter().map(|c| (c.key.clone(), c)).collect();
    let declared_facets =
        match solstone_core_facets::list_declared_facet_names(&context.journal_path) {
            Ok(names) => names.into_iter().collect::<std::collections::BTreeSet<_>>(),
            Err(error) => {
                return Ok(make_result(
                    check,
                    Status::Warn,
                    format!("couldn't fully determine; failed to read facets: {error}"),
                    Some(CANT_TELL),
                ));
            }
        };

    let mut active_facets_cache: std::collections::BTreeMap<
        String,
        std::collections::BTreeSet<String>,
    > = std::collections::BTreeMap::new();

    struct CandidateUnit {
        severity: u8,
        day: String,
        name: String,
        facet: Option<String>,
        owner_conflict_kind: Option<String>,
        lifecycle_state: String,
        has_started_owner_action: bool,
        has_committed_owner_action: bool,
        unit_retry_reprepares: bool,
    }

    let mut candidate_map: std::collections::BTreeMap<
        (String, String, Option<String>),
        CandidateUnit,
    > = std::collections::BTreeMap::new();

    let mut read_errors: Vec<String> = Vec::new();

    for day in &view.days {
        for unit in &day.why {
            let Some(lifecycle_state) = unit.lifecycle_state.as_deref() else {
                continue;
            };
            let severity = match lifecycle_state {
                "ambiguous_started" => 3u8,
                "exhausted" => 2,
                "retrying" | "stale_deferred" => 1,
                _ => continue,
            };
            let identity = solstone_core_journal_io::DailyUnitIdentity::new(
                &day.day,
                &unit.name,
                unit.facet.clone(),
            );
            let path =
                solstone_core_journal_io::daily_unit_record_path(&context.journal_path, &identity);
            let (has_started, has_committed, reprepares) =
                match solstone_core_journal_io::durability::observe_json_durable::<
                    solstone_core_journal_io::DailyUnitRecord,
                >(
                    solstone_core_journal_io::durability::ArtifactId::DailyUnits,
                    &path,
                ) {
                    solstone_core_journal_io::durability::DurableObservation::Present(rec) => {
                        let has_st = rec.has_uncommitted_started_receipt();
                        let has_cm = rec.receipts.iter().any(|r| {
                            r.get("kind").and_then(serde_json::Value::as_str)
                                == Some("owner_action")
                                && r.get("state").and_then(serde_json::Value::as_str)
                                    == Some("committed")
                        });
                        (
                            has_st,
                            has_cm,
                            solstone_core_system::daily_coverage::review_conflict_reprepares(&rec),
                        )
                    }
                    solstone_core_journal_io::durability::DurableObservation::Absent => {
                        (false, false, false)
                    }
                    solstone_core_journal_io::durability::DurableObservation::Unreadable {
                        path,
                        source,
                    } => {
                        let file_name = path
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "unknown".to_owned());
                        read_errors.push(format!(
                            "couldn't fully determine; unreadable daily unit file {file_name} on day {}: {source}",
                            day.day
                        ));
                        (false, false, false)
                    }
                    solstone_core_journal_io::durability::DurableObservation::Malformed {
                        path,
                        source,
                    } => {
                        let file_name = path
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "unknown".to_owned());
                        read_errors.push(format!(
                            "couldn't fully determine; malformed daily unit file {file_name} on day {}: {source}",
                            day.day
                        ));
                        (false, false, false)
                    }
                };

            candidate_map.insert(
                (day.day.clone(), unit.name.clone(), unit.facet.clone()),
                CandidateUnit {
                    severity,
                    day: day.day.clone(),
                    name: unit.name.clone(),
                    facet: unit.facet.clone(),
                    owner_conflict_kind: unit.owner_conflict_kind.clone(),
                    lifecycle_state: lifecycle_state.to_owned(),
                    has_started_owner_action: has_started,
                    has_committed_owner_action: has_committed,
                    unit_retry_reprepares: reprepares,
                },
            );
        }
    }

    let chronicle_dir = context.journal_path.join("chronicle");
    let entries = match std::fs::read_dir(&chronicle_dir) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Warn,
                format!("couldn't fully determine; failed to read chronicle: {error}"),
                Some(CANT_TELL),
            ));
        }
    };
    if let Some(entries) = entries {
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    return Ok(make_result(
                        check,
                        Status::Warn,
                        format!("couldn't fully determine; failed to read chronicle entry: {err}"),
                        Some(CANT_TELL),
                    ));
                }
            };
            let day = entry.file_name().to_string_lossy().to_string();
            if solstone_core_time::parse_day(&day).is_none() {
                continue;
            }
            let units_dir = entry.path().join("health/daily-units");
            let unit_entries = match std::fs::read_dir(&units_dir) {
                Ok(ue) => ue,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => {
                    return Ok(make_result(
                        check,
                        Status::Warn,
                        format!(
                            "couldn't fully determine; failed to read daily-units directory for {day}: {err}"
                        ),
                        Some(CANT_TELL),
                    ));
                }
            };
            for unit_entry in unit_entries {
                let unit_entry = match unit_entry {
                    Ok(ue) => ue,
                    Err(err) => {
                        return Ok(make_result(
                            check,
                            Status::Warn,
                            format!(
                                "couldn't fully determine; failed to read daily-unit entry for {day}: {err}"
                            ),
                            Some(CANT_TELL),
                        ));
                    }
                };
                let file_name = unit_entry.file_name().to_string_lossy().to_string();
                if !file_name.ends_with(".json") {
                    continue;
                }
                let observation = solstone_core_journal_io::durability::observe_json_durable::<
                    solstone_core_journal_io::DailyUnitRecord,
                >(
                    solstone_core_journal_io::durability::ArtifactId::DailyUnits,
                    &unit_entry.path(),
                );
                let record = match observation {
                    solstone_core_journal_io::durability::DurableObservation::Absent => {
                        continue;
                    }
                    solstone_core_journal_io::durability::DurableObservation::Unreadable {
                        path,
                        source,
                    } => {
                        let file_name = path
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "unknown".to_owned());
                        read_errors.push(format!(
                            "couldn't fully determine; unreadable daily unit file {file_name} on day {day}: {source}"
                        ));
                        continue;
                    }
                    solstone_core_journal_io::durability::DurableObservation::Malformed {
                        path,
                        source,
                    } => {
                        let file_name = path
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "unknown".to_owned());
                        read_errors.push(format!(
                            "couldn't fully determine; malformed daily unit file {file_name} on day {day}: {source}"
                        ));
                        continue;
                    }
                    solstone_core_journal_io::durability::DurableObservation::Present(r) => r,
                };

                if record.version != solstone_core_journal_io::DAILY_UNIT_RECORD_VERSION {
                    read_errors.push(format!(
                        "couldn't read daily unit file {file_name} on day {day}: unsupported record version {}",
                        record.version
                    ));
                    continue;
                }
                if record.identity.day != day
                    || record.identity.name.is_empty()
                    || record.identity.facet.as_deref() == Some("")
                    || solstone_core_journal_io::daily_unit_record_path(
                        &context.journal_path,
                        &record.identity,
                    ) != unit_entry.path()
                {
                    read_errors.push(format!(
                        "couldn't read daily unit file {file_name} on day {day}: record identity does not match its path"
                    ));
                    continue;
                }
                if record.identity.name == "daily_schedule" {
                    continue;
                }

                let is_candidate = record.status
                    == solstone_core_journal_io::DailyUnitStatus::Conflicting
                    || record.has_uncommitted_started_receipt();
                if !is_candidate {
                    continue;
                }

                let Some(cfg) = configs_map.get(&record.identity.name) else {
                    continue;
                };
                let multi_facet =
                    cfg.metadata.get("multi_facet") == Some(&serde_json::Value::Bool(true));
                if multi_facet != record.identity.facet.is_some() {
                    continue;
                }

                if let Some(facet) = record.identity.facet.as_deref() {
                    if !declared_facets.contains(facet) {
                        continue;
                    }
                    let always = cfg
                        .metadata
                        .get("always")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    if !always {
                        let active = match active_facets_cache.entry(day.clone()) {
                            std::collections::btree_map::Entry::Occupied(o) => o.get().clone(),
                            std::collections::btree_map::Entry::Vacant(v) => {
                                match solstone_core_system::activity_state::active_facets_checked(
                                    &context.journal_path,
                                    &day,
                                ) {
                                    Ok(set) => v.insert(set).clone(),
                                    Err(err) => {
                                        return Ok(make_result(
                                            check,
                                            Status::Warn,
                                            format!(
                                                "couldn't fully determine; failed to check active facets for {day}: {err}"
                                            ),
                                            Some(CANT_TELL),
                                        ));
                                    }
                                }
                            }
                        };
                        if !active.contains(facet) {
                            continue;
                        }
                    }
                }

                // Only compact stop candidates pay for evidence coverage.
                // The ordinary reader owns reuse and applicability semantics.
                let coverage = match solstone_core_system::daily_coverage::read_unit_coverage(
                    &context.journal_path,
                    &day,
                    cfg,
                    record.identity.facet.as_deref(),
                ) {
                    Ok(unit) if unit.state.is_owed() => unit,
                    Ok(_) => continue,
                    Err(error) => {
                        read_errors.push(format!(
                            "couldn't determine daily unit coverage on {day}: {error}"
                        ));
                        continue;
                    }
                };

                let has_committed = record.receipts.iter().any(|r| {
                    r.get("kind").and_then(serde_json::Value::as_str) == Some("owner_action")
                        && r.get("state").and_then(serde_json::Value::as_str) == Some("committed")
                });
                let same_revision = record.evidence_revision == coverage.evidence_revision
                    && record.contract_digest == coverage.contract_digest;

                let lifecycle_state = if record.has_uncommitted_started_receipt() {
                    "ambiguous_started"
                } else if !same_revision && !has_committed {
                    // The planner starts a new budget when evidence or contract changes.
                    "retrying"
                } else {
                    match solstone_core_system::daily_coverage::conflict_recovery(
                        &record,
                        &today,
                        &context.journal_path,
                    ) {
                        solstone_core_system::daily_coverage::ConflictRecovery::Exhausted => {
                            "exhausted"
                        }
                        solstone_core_system::daily_coverage::ConflictRecovery::StaleDeferred => {
                            "stale_deferred"
                        }
                        solstone_core_system::daily_coverage::ConflictRecovery::Open => "retrying",
                    }
                };

                let severity = match lifecycle_state {
                    "ambiguous_started" => 3u8,
                    "exhausted" => 2,
                    "retrying" | "stale_deferred" => 1,
                    _ => continue,
                };

                let has_started = record.has_uncommitted_started_receipt();
                candidate_map
                    .entry((
                        day.clone(),
                        record.identity.name.clone(),
                        record.identity.facet.clone(),
                    ))
                    .or_insert_with(|| CandidateUnit {
                        severity,
                        day: day.clone(),
                        name: record.identity.name.clone(),
                        facet: record.identity.facet.clone(),
                        owner_conflict_kind: record.owner_conflict_kind.clone(),
                        lifecycle_state: lifecycle_state.to_owned(),
                        has_started_owner_action: has_started,
                        has_committed_owner_action: has_committed,
                        unit_retry_reprepares:
                            solstone_core_system::daily_coverage::review_conflict_reprepares(
                                &record,
                            ),
                    });
            }
        }
    }

    let winning_candidate = candidate_map.values().max_by(|left, right| {
        left.severity
            .cmp(&right.severity)
            .then(right.day.cmp(&left.day))
    });

    if view.pending_days == 0
        && view.stuck_days == 0
        && candidate_map.is_empty()
        && read_errors.is_empty()
    {
        let capped = view
            .days
            .iter()
            .filter(|day| day.capped_daily.is_some())
            .count();
        let suffix = unfinished_suffix(&view.days);

        if capped == 0 {
            return Ok(make_result(
                check,
                Status::Ok,
                format!("caught up{suffix}"),
                None::<String>,
            ));
        } else {
            return Ok(make_result(
                check,
                Status::Warn,
                format!("caught up; {capped} day(s) completed with capped daily unit(s){suffix}"),
                None::<String>,
            ));
        }
    }

    let suffix = unfinished_suffix(&view.days);
    let mut detail = if view.pending_days == 0 && view.stuck_days == 0 {
        if let Some(candidate) = winning_candidate {
            let mut d = format!("daily unit outside the recent backlog on {}", candidate.day);
            if !read_errors.is_empty() {
                d.push_str(&format!("; {}", read_errors.join("; ")));
            }
            d
        } else if !read_errors.is_empty() {
            read_errors.join("; ")
        } else {
            "0 day(s) pending, 0 day(s) stuck".to_owned()
        }
    } else {
        let mut d = format!(
            "{} day(s) pending, {} day(s) stuck",
            view.pending_days, view.stuck_days
        );
        if let Some(day) = view.oldest_pending_day {
            d.push_str(&format!("; oldest outstanding {day}"));
        }
        if !read_errors.is_empty() {
            d.push_str(&format!("; {}", read_errors.join("; ")));
        }
        d
    };
    if view.pending_days != 0
        || view.stuck_days != 0
        || (winning_candidate.is_none() && !read_errors.is_empty())
    {
        detail.push_str(&suffix);
    }

    let location = |day: &str, facet: Option<&str>| match facet
        .map(str::trim)
        .filter(|facet| !facet.is_empty())
    {
        Some(facet) => format!("{day}/{facet}"),
        None => day.to_owned(),
    };
    let reason = |kind: Option<&str>| kind.map_or(String::new(), |k| format!(" with reason {k}"));
    let unit_retry = |candidate: &CandidateUnit| {
        let opt_facet = match candidate.facet.as_deref().filter(|facet| !facet.is_empty()) {
            Some(facet) => format!(" --facet {}", shell_argument(facet)),
            None => String::new(),
        };
        format!(
            "solstone journal reprocess {} --unit {}{opt_facet}",
            candidate.day,
            shell_argument(&candidate.name)
        )
    };

    let fix = match winning_candidate {
        Some(candidate)
            if candidate.lifecycle_state == "ambiguous_started"
                || candidate.has_started_owner_action =>
        {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            let change = if candidate.name == "entities:entities_review" {
                "an entity review change"
            } else {
                "a change"
            };
            format!(
                "{} may have started {change} but did not confirm completion on {loc}; resolve the in-progress change before reprocessing",
                candidate.name
            )
        }
        Some(candidate)
            if candidate.has_committed_owner_action
                && candidate.unit_retry_reprepares
                && candidate.day.as_str() < today.as_str() =>
        {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            format!(
                "{} stopped on {loc}{}; run {}",
                candidate.name,
                reason(candidate.owner_conflict_kind.as_deref()),
                unit_retry(candidate)
            )
        }
        Some(candidate) if candidate.has_committed_owner_action => {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            format!(
                "{} recorded a change on {loc}; resolve the recorded change before that day is redone",
                candidate.name
            )
        }
        Some(candidate) if candidate.lifecycle_state == "exhausted" => {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            if candidate.day.as_str() >= today.as_str() {
                format!("{} stopped on {loc}{}; resolve the conflict before reprocessing a past day",
                    candidate.name, reason(candidate.owner_conflict_kind.as_deref()))
            } else {
                // A unit retry takes only the conflicts a fresh preparation can
                // clear; any other stopped unit is redone with its whole day.
                let command = if candidate.unit_retry_reprepares {
                    unit_retry(candidate)
                } else {
                    format!("solstone journal reprocess {} --from-scratch", candidate.day)
                };
                format!(
                    "{} stopped on {loc}{} after its automatic retry; run {command}",
                    candidate.name,
                    reason(candidate.owner_conflict_kind.as_deref()),
                )
            }
        }
        Some(candidate) if candidate.lifecycle_state == "stale_deferred" => {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            format!(
                "{} stopped on {loc}{}; it can retry on a later run after the local day changes",
                candidate.name,
                reason(candidate.owner_conflict_kind.as_deref())
            )
        }
        Some(candidate) if candidate.lifecycle_state == "retrying" => {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            format!(
                "{} stopped on {loc}{}; it can retry on the next eligible run",
                candidate.name,
                reason(candidate.owner_conflict_kind.as_deref())
            )
        }
        _ if !read_errors.is_empty() => CANT_TELL.to_owned(),
        _ => "open health to see waiting work and any spending confirmation; reprocess a past day to give it priority after today".to_owned(),
    };
    Ok(make_result(check, Status::Warn, detail, Some(fix)))
}
