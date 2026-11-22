// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{BacklogDays, Check, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
const CANT_TELL: &str = "journal.cli.doctor.journal.caught.up.repair";

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

fn append_unfinished(detail: &mut String, days: &[solstone_core_system_health::BacklogDay]) {
    let unfinished = solstone_core_system_health::aggregate_unfinished_from_days(days);
    if unfinished.activities > 0 {
        detail.push_str("; ");
        detail.push_str(&crate::vocabulary::ReportForm::Human.counted(
            "journal.cli.doctor.backlog.unfinished",
            &[
                ("activity_count", unfinished.activities as u64),
                ("day_count", unfinished.day_count as u64),
            ],
            &[
                ("activities", &unfinished.activities),
                ("days", &unfinished.day_count),
            ],
        ));
    }
}

/// Every result read from the backlog carries the days behind its counts, so
/// a reader has the set rather than deriving it.
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
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
            let bands = report
                .bands
                .iter()
                .enumerate()
                .map(|(i, band)| {
                    context.report_form.counted(
                        "journal.cli.doctor.backlog.band",
                        &[
                            ("recording_count", band.segments_complete),
                            ("output_count", band.daily_units_complete),
                            ("activity_count", band.activities_owed),
                            ("request_count", band.owner_requests),
                        ],
                        &[
                            ("band", &(i + 1)),
                            ("recordings", &band.segments_complete),
                            ("recordings_left", &band.segments_owed),
                            ("outputs", &band.daily_units_complete),
                            ("outputs_left", &band.daily_units_owed),
                            ("activities", &band.activities_owed),
                            ("requests", &band.owner_requests),
                            ("permission", &band.permission_held),
                            ("prerequisites", &band.prerequisite_held),
                        ],
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            let holds = report
                .holds
                .iter()
                .map(|hold| match &hold.retry_unit {
                    Some(unit) => copy::display(
                        "journal.cli.doctor.backlog.hold.retry",
                        [
                            ("day", &hold.day),
                            ("reason", &hold.reason),
                            ("unit", &unit),
                        ],
                    ),
                    None => format!("{}: {}", hold.day, hold.reason),
                })
                .collect::<Vec<_>>()
                .join("; ");
            let missing_date = copy::text("journal.cli.doctor.backlog.empty.date");
            let oldest = report.oldest_day.as_deref().unwrap_or(&missing_date);
            let newest = report.newest_day.as_deref().unwrap_or(&missing_date);
            (
                match (report.complete, fresh, report.holds.is_empty()) {
                    (true, true, true) => context.report_form.counted(
                        "journal.cli.doctor.backlog.inventory.whole.current.clear",
                        &[("quantity", report.days_total as u64)],
                        &[
                            ("inspected", &report.days_inspected),
                            ("total", &report.days_total),
                            ("oldest", &oldest),
                            ("newest", &newest),
                            ("as_of", &report.as_of_ms),
                            ("bands", &bands),
                        ],
                    ),
                    (true, true, false) => context.report_form.counted(
                        "journal.cli.doctor.backlog.inventory.whole.current",
                        &[("quantity", report.days_total as u64)],
                        &[
                            ("inspected", &report.days_inspected),
                            ("total", &report.days_total),
                            ("oldest", &oldest),
                            ("newest", &newest),
                            ("as_of", &report.as_of_ms),
                            ("bands", &bands),
                            ("holds", &holds),
                        ],
                    ),
                    (true, false, true) => context.report_form.counted(
                        "journal.cli.doctor.backlog.inventory.whole.last.clear",
                        &[("quantity", report.days_total as u64)],
                        &[
                            ("inspected", &report.days_inspected),
                            ("total", &report.days_total),
                            ("oldest", &oldest),
                            ("newest", &newest),
                            ("as_of", &report.as_of_ms),
                            ("bands", &bands),
                        ],
                    ),
                    (true, false, false) => context.report_form.counted(
                        "journal.cli.doctor.backlog.inventory.whole.last",
                        &[("quantity", report.days_total as u64)],
                        &[
                            ("inspected", &report.days_inspected),
                            ("total", &report.days_total),
                            ("oldest", &oldest),
                            ("newest", &newest),
                            ("as_of", &report.as_of_ms),
                            ("bands", &bands),
                            ("holds", &holds),
                        ],
                    ),
                    (false, true, true) => context.report_form.counted(
                        "journal.cli.doctor.backlog.inventory.partial.current.clear",
                        &[("quantity", report.days_total as u64)],
                        &[
                            ("inspected", &report.days_inspected),
                            ("total", &report.days_total),
                            ("oldest", &oldest),
                            ("newest", &newest),
                            ("as_of", &report.as_of_ms),
                            ("bands", &bands),
                        ],
                    ),
                    (false, true, false) => context.report_form.counted(
                        "journal.cli.doctor.backlog.inventory.partial.current",
                        &[("quantity", report.days_total as u64)],
                        &[
                            ("inspected", &report.days_inspected),
                            ("total", &report.days_total),
                            ("oldest", &oldest),
                            ("newest", &newest),
                            ("as_of", &report.as_of_ms),
                            ("bands", &bands),
                            ("holds", &holds),
                        ],
                    ),
                    (false, false, true) => context.report_form.counted(
                        "journal.cli.doctor.backlog.inventory.partial.last.clear",
                        &[("quantity", report.days_total as u64)],
                        &[
                            ("inspected", &report.days_inspected),
                            ("total", &report.days_total),
                            ("oldest", &oldest),
                            ("newest", &newest),
                            ("as_of", &report.as_of_ms),
                            ("bands", &bands),
                        ],
                    ),
                    (false, false, false) => context.report_form.counted(
                        "journal.cli.doctor.backlog.inventory.partial.last",
                        &[("quantity", report.days_total as u64)],
                        &[
                            ("inspected", &report.days_inspected),
                            ("total", &report.days_total),
                            ("oldest", &oldest),
                            ("newest", &newest),
                            ("as_of", &report.as_of_ms),
                            ("bands", &bands),
                            ("holds", &holds),
                        ],
                    ),
                },
                fresh
                    && report.complete
                    && report.unreadable.is_empty()
                    && report.holds.is_empty()
                    && !owed,
            )
        }
        Ok(None) => (
            copy::text("journal.cli.doctor.journal.caught.up.inventory.waiting"),
            false,
        ),
        Err(error) => (
            copy::display(
                "journal.cli.doctor.journal.caught.up.inventory.unavailable",
                [("error", &error)],
            ),
            false,
        ),
    };
    let mut days = None;
    let mut result = evaluate(context, check, &mut days, complete)?;
    result.backlog_days = days;
    if !complete {
        if result.status == Status::Ok {
            result.status = Status::Warn;
        }
        if result.fix.is_none() {
            result.fix = Some(copy::text(
                "journal.cli.doctor.journal.caught.up.inventory.fix",
            ));
        }
    }
    result.detail.push_str("; ");
    result.detail.push_str(&summary);
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

fn evaluate(
    context: &CheckContext,
    check: Check,
    days: &mut Option<BacklogDays>,
    complete: bool,
) -> RunnerResult {
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
                copy::display(
                    "journal.cli.doctor.journal.caught.up.backlog.unavailable",
                    [("error", &error)],
                ),
                Some(copy::text(CANT_TELL)),
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
        let mut detail = context.report_form.counted(
            "journal.cli.doctor.backlog.unknown.count",
            &[("quantity", unknown as u64)],
            &[("days", &unknown)],
        );
        append_unfinished(&mut detail, &view.days);
        return Ok(make_result(
            check,
            Status::Warn,
            detail,
            Some(copy::text(CANT_TELL)),
        ));
    }

    let today = solstone_core_system::daily_coverage::local_day(&context.journal_path, context.now);
    let (talent_root, apps_root) = match context.payload_root.as_ref() {
        Some(payload) => (
            payload.join("solstone/talent"),
            payload.join("solstone/apps"),
        ),
        None => match solstone_core_system::daily_coverage::package_roots_in(
            context.report_form.locale(),
        ) {
            Ok(roots) => roots,
            Err(err) => {
                return Ok(make_result(
                    check,
                    Status::Warn,
                    copy::display(
                        "journal.cli.doctor.journal.caught.up.definitions.unavailable",
                        [("error", &err)],
                    ),
                    Some(copy::text(CANT_TELL)),
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
                copy::display(
                    "journal.cli.doctor.journal.caught.up.configs.unavailable",
                    [("error", &err)],
                ),
                Some(copy::text(CANT_TELL)),
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
                    copy::display(
                        "journal.cli.doctor.journal.caught.up.facets.unavailable",
                        [("error", &error)],
                    ),
                    Some(copy::text(CANT_TELL)),
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
                        read_errors.push(copy::display(
                            "journal.cli.doctor.journal.caught.up.unit.unreadable",
                            [
                                ("file_name", &file_name),
                                ("day", &day.day),
                                ("source", &source),
                            ],
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
                        read_errors.push(copy::display(
                            "journal.cli.doctor.journal.caught.up.unit.invalid",
                            [
                                ("file_name", &file_name),
                                ("day", &day.day),
                                ("source", &source),
                            ],
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
                copy::display(
                    "journal.cli.doctor.journal.caught.up.chronicle.unavailable",
                    [("error", &error)],
                ),
                Some(copy::text(CANT_TELL)),
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
                        copy::display(
                            "journal.cli.doctor.journal.caught.up.chronicle.entry.unavailable",
                            [("error", &err)],
                        ),
                        Some(copy::text(CANT_TELL)),
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
                        copy::display(
                            "journal.cli.doctor.journal.caught.up.units.unavailable",
                            [("day", &day), ("error", &err)],
                        ),
                        Some(copy::text(CANT_TELL)),
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
                            copy::display(
                                "journal.cli.doctor.journal.caught.up.units.entry.unavailable",
                                [("day", &day), ("error", &err)],
                            ),
                            Some(copy::text(CANT_TELL)),
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
                        read_errors.push(copy::display(
                            "journal.cli.doctor.journal.caught.up.unit.unreadable",
                            [
                                ("file_name", &file_name),
                                ("day", &day),
                                ("source", &source),
                            ],
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
                        read_errors.push(copy::display(
                            "journal.cli.doctor.journal.caught.up.unit.invalid",
                            [
                                ("file_name", &file_name),
                                ("day", &day),
                                ("source", &source),
                            ],
                        ));
                        continue;
                    }
                    solstone_core_journal_io::durability::DurableObservation::Present(r) => r,
                };

                if record.version != solstone_core_journal_io::DAILY_UNIT_RECORD_VERSION {
                    read_errors.push(copy::display(
                        "journal.cli.doctor.journal.caught.up.unit.version",
                        [
                            ("file_name", &file_name),
                            ("day", &day),
                            ("version", &record.version),
                        ],
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
                    read_errors.push(copy::display(
                        "journal.cli.doctor.journal.caught.up.unit.identity",
                        [("file_name", &file_name), ("day", &day)],
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
                                            copy::display(
                                                "journal.cli.doctor.journal.caught.up.facets.day.unavailable",
                                                [("day", &day), ("error", &err)],
                                            ),
                                            Some(copy::text(CANT_TELL)),
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
                        read_errors.push(copy::display(
                            "journal.cli.doctor.journal.caught.up.coverage.unavailable",
                            [("day", &day), ("error", &error)],
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
        let units = view
            .days
            .iter()
            .filter_map(|day| day.capped_daily.as_ref())
            .fold(0usize, |count, day| count.saturating_add(day.count));
        let mut detail = match (complete, capped == 0) {
            (true, true) => copy::text("journal.cli.doctor.backlog.caught"),
            (true, false) => context.report_form.counted(
                "journal.cli.doctor.backlog.caught.capped",
                &[("day_count", capped as u64), ("unit_count", units as u64)],
                &[("days", &capped)],
            ),
            (false, true) => copy::text("journal.cli.doctor.backlog.checked"),
            (false, false) => context.report_form.counted(
                "journal.cli.doctor.backlog.checked.capped",
                &[("day_count", capped as u64), ("unit_count", units as u64)],
                &[("days", &capped)],
            ),
        };
        append_unfinished(&mut detail, &view.days);
        let status = if capped == 0 {
            Status::Ok
        } else {
            Status::Warn
        };
        return Ok(make_result(check, status, detail, None::<String>));
    }

    let mut detail = if view.pending_days == 0 && view.stuck_days == 0 {
        if let Some(candidate) = winning_candidate {
            let mut d = copy::display(
                "journal.cli.doctor.journal.caught.up.outside.backlog",
                [("day", &candidate.day)],
            );
            if !read_errors.is_empty() {
                d.push_str(&format!("; {}", read_errors.join("; ")));
            }
            d
        } else if !read_errors.is_empty() {
            read_errors.join("; ")
        } else {
            context.report_form.counted(
                "journal.cli.doctor.backlog.pending.counts",
                &[
                    ("pending_count", view.pending_days as u64),
                    ("stuck_count", view.stuck_days as u64),
                ],
                &[("pending", &view.pending_days), ("stuck", &view.stuck_days)],
            )
        }
    } else {
        let mut d = match &view.oldest_pending_day {
            Some(day) => context.report_form.counted(
                "journal.cli.doctor.backlog.pending.oldest",
                &[
                    ("pending_count", view.pending_days as u64),
                    ("stuck_count", view.stuck_days as u64),
                ],
                &[
                    ("pending", &view.pending_days),
                    ("stuck", &view.stuck_days),
                    ("day", &day),
                ],
            ),
            None => context.report_form.counted(
                "journal.cli.doctor.backlog.pending.counts",
                &[
                    ("pending_count", view.pending_days as u64),
                    ("stuck_count", view.stuck_days as u64),
                ],
                &[("pending", &view.pending_days), ("stuck", &view.stuck_days)],
            ),
        };
        if !read_errors.is_empty() {
            d.push_str(&format!("; {}", read_errors.join("; ")));
        }
        d
    };
    if view.pending_days != 0
        || view.stuck_days != 0
        || (winning_candidate.is_none() && !read_errors.is_empty())
    {
        append_unfinished(&mut detail, &view.days);
    }

    let location = |day: &str, facet: Option<&str>| match facet
        .map(str::trim)
        .filter(|facet| !facet.is_empty())
    {
        Some(facet) => format!("{day}/{facet}"),
        None => day.to_owned(),
    };
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
            if candidate.name == "entities:entities_review" {
                copy::display(
                    "journal.cli.doctor.backlog.review.started",
                    [("name", &candidate.name), ("location", &loc)],
                )
            } else {
                copy::display(
                    "journal.cli.doctor.backlog.change.started",
                    [("name", &candidate.name), ("location", &loc)],
                )
            }
        }
        Some(candidate)
            if candidate.has_committed_owner_action
                && candidate.unit_retry_reprepares
                && candidate.day.as_str() < today.as_str() =>
        {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            match candidate.owner_conflict_kind.as_deref() {
                Some(reason) => copy::display(
                    "journal.cli.doctor.backlog.retry.reason",
                    [
                        ("name", &candidate.name),
                        ("location", &loc),
                        ("command", &(unit_retry(candidate))),
                        ("reason", &reason),
                    ],
                ),
                None => copy::display(
                    "journal.cli.doctor.backlog.retry",
                    [
                        ("name", &candidate.name),
                        ("location", &loc),
                        ("command", &(unit_retry(candidate))),
                    ],
                ),
            }
        }
        Some(candidate) if candidate.has_committed_owner_action => {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            copy::display(
                "journal.cli.doctor.backlog.change.recorded",
                [("name", &candidate.name), ("location", &loc)],
            )
        }
        Some(candidate) if candidate.lifecycle_state == "exhausted" => {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            if candidate.day.as_str() >= today.as_str() {
                match candidate.owner_conflict_kind.as_deref() {
                    Some(reason) => copy::display(
                        "journal.cli.doctor.backlog.resolve.reason",
                        [
                            ("name", &candidate.name),
                            ("location", &loc),
                            ("reason", &reason),
                        ],
                    ),
                    None => copy::display(
                        "journal.cli.doctor.backlog.resolve",
                        [("name", &candidate.name), ("location", &loc)],
                    ),
                }
            } else {
                // A unit retry takes only the conflicts a fresh preparation can
                // clear; any other stopped unit is redone with its whole day.
                let command = if candidate.unit_retry_reprepares {
                    unit_retry(candidate)
                } else {
                    format!(
                        "solstone journal reprocess {} --from-scratch",
                        candidate.day
                    )
                };
                match candidate.owner_conflict_kind.as_deref() {
                    Some(reason) => copy::display(
                        "journal.cli.doctor.backlog.exhausted.reason",
                        [
                            ("name", &candidate.name),
                            ("location", &loc),
                            ("command", &command),
                            ("reason", &reason),
                        ],
                    ),
                    None => copy::display(
                        "journal.cli.doctor.backlog.exhausted",
                        [
                            ("name", &candidate.name),
                            ("location", &loc),
                            ("command", &command),
                        ],
                    ),
                }
            }
        }
        Some(candidate) if candidate.lifecycle_state == "stale_deferred" => {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            match candidate.owner_conflict_kind.as_deref() {
                Some(reason) => copy::display(
                    "journal.cli.doctor.backlog.deferred.reason",
                    [
                        ("name", &candidate.name),
                        ("location", &loc),
                        ("reason", &reason),
                    ],
                ),
                None => copy::display(
                    "journal.cli.doctor.backlog.deferred",
                    [("name", &candidate.name), ("location", &loc)],
                ),
            }
        }
        Some(candidate) if candidate.lifecycle_state == "retrying" => {
            let loc = location(&candidate.day, candidate.facet.as_deref());
            match candidate.owner_conflict_kind.as_deref() {
                Some(reason) => copy::display(
                    "journal.cli.doctor.backlog.eligible.reason",
                    [
                        ("name", &candidate.name),
                        ("location", &loc),
                        ("reason", &reason),
                    ],
                ),
                None => copy::display(
                    "journal.cli.doctor.backlog.eligible",
                    [("name", &candidate.name), ("location", &loc)],
                ),
            }
        }
        _ if !read_errors.is_empty() => copy::text(CANT_TELL),
        _ => copy::text("journal.cli.doctor.journal.caught.up.inspect"),
    };
    Ok(make_result(check, Status::Warn, detail, Some(fix)))
}
