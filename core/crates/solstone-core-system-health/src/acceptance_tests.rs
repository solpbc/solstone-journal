// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use chrono::{Duration, TimeZone, Utc};
    use serde_json::{Map, json};
    use tempfile::TempDir;

    use crate::{
        BACKLOG_STATE_COMPLETE, FilesystemHealthLogSource, FilesystemSegmentSource, HealthError,
        HealthLogSource, SummaryFreshness, UNFINISHED_TEMPLATE_MANY_DAYS,
        UNFINISHED_TEMPLATE_MANY_ONE_DAY, UNFINISHED_TEMPLATE_ONE, aggregate_unfinished_activities,
        format_summary_age, parse_summary_time, read_backlog_view, select_unfinished_template,
        summary_freshness,
    };

    fn configure_daily_work(journal: &Path, enabled: Option<&str>) {
        let (talent, apps) = solstone_core_system::daily_coverage::package_roots().unwrap();
        let configs =
            solstone_core_system::daily_coverage::daily_configs(journal, &talent, &apps).unwrap();
        let overrides = configs
            .into_iter()
            .map(|config| {
                let key = match config.key.split_once(':') {
                    Some((app, name)) => format!("talent.{app}.{name}"),
                    None => format!("talent.system.{}", config.key),
                };
                (
                    key,
                    json!({"disabled": enabled != Some(config.key.as_str())}),
                )
            })
            .collect::<serde_json::Map<String, serde_json::Value>>();
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            serde_json::to_vec(
                &json!({"identity":{"timezone":"UTC"},"talent_overrides":overrides}),
            )
            .unwrap(),
        )
        .unwrap();
    }

    fn write_run_log(root: &Path, day: &str, file: &str, lines: &[&str]) {
        let dir = root.join("chronicle").join(day).join("health");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), lines.join("\n") + "\n").unwrap();
    }

    #[test]
    fn acceptance_1_completed_day_unfinished_activities_fold() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        configure_daily_work(root, None);

        let day = "20990101";
        let day_dir = root.join("chronicle").join(day);
        fs::create_dir_all(&day_dir).unwrap();

        // Control journal without fail row
        let temp_control = TempDir::new().unwrap();
        let root_control = temp_control.path();
        configure_daily_work(root_control, None);
        fs::create_dir_all(root_control.join("chronicle").join(day)).unwrap();

        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let view_control = read_backlog_view(
            &FilesystemHealthLogSource::new(root_control),
            &FilesystemSegmentSource,
            root_control,
            1,
            now,
        )
        .unwrap();

        // Failed activity run log record
        let record = json!({
            "ts": 10_000,
            "event": "talent.fail",
            "mode": "activity",
            "name": "entities_observe",
            "facet": "work",
            "activity": "meeting-1",
            "reason_code": "timeout"
        });
        write_run_log(root, day, "001.jsonl", &[&record.to_string()]);

        let view = read_backlog_view(
            &FilesystemHealthLogSource::new(root),
            &FilesystemSegmentSource,
            root,
            1,
            now,
        )
        .unwrap();

        // Pending/stuck/oldest match the control journal without the fail row
        assert_eq!(view.pending_days, view_control.pending_days);
        assert_eq!(view.stuck_days, view_control.stuck_days);
        assert_eq!(view.oldest_pending_day, view_control.oldest_pending_day);

        assert_eq!(view.days.len(), 1);
        let day_res = &view.days[0];
        assert_eq!(day_res.state, BACKLOG_STATE_COMPLETE);
        assert!(day_res.why.is_empty());
        assert_eq!(day_res.units, 0);
        assert!(day_res.error.is_none());

        let unfinished = day_res
            .unfinished_activities
            .as_ref()
            .expect("unfinished activities present on completed day");
        assert_eq!(unfinished.activities, 1);
        assert_eq!(unfinished.units.len(), 1);
        let u = &unfinished.units[0];
        assert_eq!(u.mode, "activity");
        assert_eq!(u.name, "entities_observe");
        assert_eq!(u.facet.as_deref(), Some("work"));
        assert_eq!(u.activity.as_deref(), Some("meeting-1"));
        assert_eq!(u.reason_code.as_deref(), Some("timeout"));
        assert!(!u.stuck, "completed-day unfinished units must not be stuck");
    }

    #[test]
    fn acceptance_completed_day_activity_combinations() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        configure_daily_work(root, None);

        let day = "20990101";
        fs::create_dir_all(root.join("chronicle").join(day)).unwrap();

        // 1. One activity, two failed talents -> activities == 1, two units
        let fail1 = json!({
            "ts": 10_000,
            "event": "talent.fail",
            "mode": "activity",
            "name": "talent_a",
            "facet": "work",
            "activity": "meeting-1",
            "reason_code": "timeout"
        });
        let fail2 = json!({
            "ts": 11_000,
            "event": "talent.fail",
            "mode": "activity",
            "name": "talent_b",
            "facet": "work",
            "activity": "meeting-1",
            "reason_code": "no_output"
        });
        write_run_log(
            root,
            day,
            "001.jsonl",
            &[&fail1.to_string(), &fail2.to_string()],
        );

        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let view = read_backlog_view(
            &FilesystemHealthLogSource::new(root),
            &FilesystemSegmentSource,
            root,
            1,
            now,
        )
        .unwrap();

        let unf = view.days[0].unfinished_activities.as_ref().unwrap();
        assert_eq!(unf.activities, 1);
        assert_eq!(unf.units.len(), 2);

        // 2. Two activities, one facet, same talent -> activities == 2, units distinguished by activity
        let fail3 = json!({
            "ts": 12_000,
            "event": "talent.fail",
            "mode": "activity",
            "name": "talent_a",
            "facet": "work",
            "activity": "meeting-2",
            "reason_code": "timeout"
        });
        write_run_log(root, day, "002.jsonl", &[&fail3.to_string()]);

        let view2 = read_backlog_view(
            &FilesystemHealthLogSource::new(root),
            &FilesystemSegmentSource,
            root,
            1,
            now,
        )
        .unwrap();
        let unf2 = view2.days[0].unfinished_activities.as_ref().unwrap();
        assert_eq!(unf2.activities, 2);
        assert_eq!(unf2.units.len(), 3);

        // 3. Two certified days with failed meeting_1 in the same facet -> aggregate is activities 2, day_count 2, oldest_day the older day
        let day_older = "20990101";
        let day_newer = "20990102";
        let temp_multi = TempDir::new().unwrap();
        let root_multi = temp_multi.path();
        configure_daily_work(root_multi, None);
        fs::create_dir_all(root_multi.join("chronicle").join(day_older)).unwrap();
        fs::create_dir_all(root_multi.join("chronicle").join(day_newer)).unwrap();
        write_run_log(
            root_multi,
            day_older,
            "001.jsonl",
            &[&json!({
                "ts": 10_000,
                "event": "talent.fail",
                "mode": "activity",
                "name": "talent_a",
                "facet": "work",
                "activity": "meeting_1",
                "reason_code": "timeout"
            })
            .to_string()],
        );
        write_run_log(
            root_multi,
            day_newer,
            "001.jsonl",
            &[&json!({
                "ts": 20_000,
                "event": "talent.fail",
                "mode": "activity",
                "name": "talent_a",
                "facet": "work",
                "activity": "meeting_1",
                "reason_code": "timeout"
            })
            .to_string()],
        );

        let view_multi = read_backlog_view(
            &FilesystemHealthLogSource::new(root_multi),
            &FilesystemSegmentSource,
            root_multi,
            2,
            now,
        )
        .unwrap();

        let json_val = serde_json::to_value(&view_multi).unwrap();
        let agg = aggregate_unfinished_activities(json_val.as_object().unwrap());
        assert_eq!(agg.activities, 2);
        assert_eq!(agg.day_count, 2);
        assert_eq!(agg.oldest_day.as_deref(), Some(day_older));
    }

    #[test]
    fn acceptance_later_talent_complete_clears_or_keeps_units() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        configure_daily_work(root, None);

        let day = "20990101";
        fs::create_dir_all(root.join("chronicle").join(day)).unwrap();

        // Fail talent_a on meeting-1
        let fail = json!({
            "ts": 10_000,
            "event": "talent.fail",
            "mode": "activity",
            "name": "talent_a",
            "facet": "work",
            "activity": "meeting-1",
            "reason_code": "timeout"
        });
        // Later talent.complete for same talent on DIFFERENT activity meeting-2 -> leaves original unit
        let complete_other = json!({
            "ts": 11_000,
            "event": "talent.complete",
            "mode": "activity",
            "name": "talent_a",
            "facet": "work",
            "activity": "meeting-2"
        });
        write_run_log(
            root,
            day,
            "001.jsonl",
            &[&fail.to_string(), &complete_other.to_string()],
        );

        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let view = read_backlog_view(
            &FilesystemHealthLogSource::new(root),
            &FilesystemSegmentSource,
            root,
            1,
            now,
        )
        .unwrap();
        let unf = view.days[0].unfinished_activities.as_ref().unwrap();
        assert_eq!(unf.activities, 1);
        assert_eq!(unf.units.len(), 1);
        assert_eq!(unf.units[0].activity.as_deref(), Some("meeting-1"));

        // Now add later talent.complete for the SAME unit (meeting-1) -> removes unfinished_activities
        let complete_same = json!({
            "ts": 12_000,
            "event": "talent.complete",
            "mode": "activity",
            "name": "talent_a",
            "facet": "work",
            "activity": "meeting-1"
        });
        write_run_log(root, day, "002.jsonl", &[&complete_same.to_string()]);

        let view2 = read_backlog_view(
            &FilesystemHealthLogSource::new(root),
            &FilesystemSegmentSource,
            root,
            1,
            now,
        )
        .unwrap();
        assert!(view2.days[0].unfinished_activities.is_none());
        assert_eq!(view2.days[0].state, BACKLOG_STATE_COMPLETE);
    }

    #[test]
    fn acceptance_capped_and_complete_rows() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let day = "20251230";
        fs::create_dir_all(root.join("chronicle").join(day)).unwrap();

        // Enable schedule talent and write a capped daily unit record
        configure_daily_work(root, Some("schedule"));
        let coverage =
            solstone_core_system::daily_coverage::read_daily_coverage(root, day).unwrap();
        let unit = coverage
            .units
            .iter()
            .find(|unit| unit.identity.name == "schedule")
            .unwrap();
        let mut record = solstone_core_journal_io::DailyUnitRecord::new(
            unit.identity.clone(),
            &unit.evidence_revision,
            &unit.contract_digest,
        );
        record.status = solstone_core_journal_io::DailyUnitStatus::Capped;
        record.reason_code = Some("provider_request_rejected".into());
        record.failure_count = 1;
        solstone_core_journal_io::save_daily_unit_record(root, &record).unwrap();

        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let view = read_backlog_view(
            &FilesystemHealthLogSource::new(root),
            &FilesystemSegmentSource,
            root,
            1,
            now,
        )
        .unwrap();

        assert_eq!(view.days[0].state, BACKLOG_STATE_COMPLETE);
        assert_eq!(view.days[0].capped_daily.as_ref().map(|c| c.count), Some(1));
        assert!(view.days[0].unfinished_activities.is_none());
    }

    struct FailingHealthOnDayRecords;

    impl HealthLogSource for FailingHealthOnDayRecords {
        fn health_log_paths(&self, day: &str) -> Result<Vec<PathBuf>, HealthError> {
            Err(HealthError::Source(format!(
                "simulated health read error for {day}"
            )))
        }
    }

    #[test]
    fn acceptance_2_completed_day_terminal_read_failure_degrades_view() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        configure_daily_work(root, None);

        let day = "20990101";
        let day_dir = root.join("chronicle").join(day);
        fs::create_dir_all(&day_dir).unwrap();

        let failing_source = FailingHealthOnDayRecords;
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let view =
            read_backlog_view(&failing_source, &FilesystemSegmentSource, root, 1, now).unwrap();

        assert_eq!(view.days.len(), 1);
        let day_res = &view.days[0];
        assert_eq!(day_res.state, BACKLOG_STATE_COMPLETE);
        assert!(day_res.error.is_none());
        assert_eq!(view.errors.len(), 1);
        assert_eq!(view.errors[0].stage, "completed_day_terminals");
        assert!(view.degraded);
    }

    #[test]
    fn acceptance_3_summary_freshness_and_age_formatting() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();

        // Absent (None) -> Unknown
        assert_eq!(summary_freshness(None, now), SummaryFreshness::Unknown);
        // Unparseable -> Unknown
        assert_eq!(
            summary_freshness(Some("invalid-timestamp"), now),
            SummaryFreshness::Unknown
        );
        // +10 min -> Unknown
        let t_plus_10m = (now + Duration::minutes(10)).to_rfc3339();
        assert_eq!(
            summary_freshness(Some(&t_plus_10m), now),
            SummaryFreshness::Unknown
        );
        // +2 min -> Fresh
        let t_plus_2m = (now + Duration::minutes(2)).to_rfc3339();
        assert_eq!(
            summary_freshness(Some(&t_plus_2m), now),
            SummaryFreshness::Fresh
        );
        // 35h -> Fresh
        let t_35h = (now - Duration::hours(35)).to_rfc3339();
        assert_eq!(
            summary_freshness(Some(&t_35h), now),
            SummaryFreshness::Fresh
        );
        // exactly 36h -> Fresh
        let t_36h = (now - Duration::hours(36)).to_rfc3339();
        assert_eq!(
            summary_freshness(Some(&t_36h), now),
            SummaryFreshness::Fresh
        );
        // 37h -> Stale
        let t_37h = (now - Duration::hours(37)).to_rfc3339();
        assert_eq!(
            summary_freshness(Some(&t_37h), now),
            SummaryFreshness::Stale
        );

        // format_summary_age:
        // 30s -> 1 minute
        assert_eq!(format_summary_age(Duration::seconds(30)), "1 minute");
        // 59 min -> 59 minutes
        assert_eq!(format_summary_age(Duration::minutes(59)), "59 minutes");
        // 1h -> 1 hour
        assert_eq!(format_summary_age(Duration::hours(1)), "1 hour");
        // 47h -> 47 hours
        assert_eq!(format_summary_age(Duration::hours(47)), "47 hours");
        // 48h -> 2 days
        assert_eq!(format_summary_age(Duration::hours(48)), "2 days");
        // 9d -> 9 days
        assert_eq!(format_summary_age(Duration::days(9)), "9 days");

        // parse_summary_time validation
        assert_eq!(
            parse_summary_time("2026-09-22T20:00:00Z"),
            Some(Utc.with_ymd_and_hms(2026, 9, 22, 20, 0, 0).unwrap())
        );
        assert_eq!(parse_summary_time("   "), None);
    }

    #[test]
    fn acceptance_4_unfinished_template_selection() {
        // Template 1: single activity
        assert_eq!(
            select_unfinished_template(1, 1),
            Some(UNFINISHED_TEMPLATE_ONE)
        );

        // Template 2: multiple activities on 1 day
        assert_eq!(
            select_unfinished_template(2, 1),
            Some(UNFINISHED_TEMPLATE_MANY_ONE_DAY)
        );

        // Template 3: multiple activities across multiple days
        assert_eq!(
            select_unfinished_template(3, 2),
            Some(UNFINISHED_TEMPLATE_MANY_DAYS)
        );

        // 0 activities -> None
        assert_eq!(select_unfinished_template(0, 0), None);

        // Test aggregate_unfinished_activities over backlog json
        let mut backlog_map = Map::new();
        backlog_map.insert(
            "days".to_owned(),
            json!([
                {
                    "day": "20990102",
                    "unfinished_activities": {
                        "activities": 2,
                        "units": []
                    }
                },
                {
                    "day": "20990101",
                    "unfinished_activities": {
                        "activities": 1,
                        "units": []
                    }
                }
            ]),
        );
        let agg = aggregate_unfinished_activities(&backlog_map);
        assert_eq!(agg.activities, 3);
        assert_eq!(agg.day_count, 2);
        assert_eq!(agg.oldest_day.as_deref(), Some("20990101"));
    }

    #[test]
    fn acceptance_5_indexer_phase_fold_winner_and_tiebreaker() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        configure_daily_work(root, None);

        let day1 = "20990101";
        let day2 = "20990102";
        fs::create_dir_all(root.join("chronicle").join(day1)).unwrap();
        fs::create_dir_all(root.join("chronicle").join(day2)).unwrap();

        // Control journal without indexer rows
        let temp_control = TempDir::new().unwrap();
        let root_control = temp_control.path();
        configure_daily_work(root_control, None);
        fs::create_dir_all(root_control.join("chronicle").join(day1)).unwrap();
        fs::create_dir_all(root_control.join("chronicle").join(day2)).unwrap();
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let view_control = read_backlog_view(
            &FilesystemHealthLogSource::new(root_control),
            &FilesystemSegmentSource,
            root_control,
            2,
            now,
        )
        .unwrap();

        // 1. A failed phase.complete is the winner when a later row is skipped: true, success: false
        let row_failed = json!({
            "ts": 1_000,
            "event": "phase.complete",
            "phase": "indexer",
            "success": false,
            "reason_code": "failed"
        });
        let row_skipped = json!({
            "ts": 2_000,
            "event": "phase.complete",
            "phase": "indexer",
            "skipped": true,
            "success": false
        });
        // Integer success and missing success are ignored
        let row_int = json!({
            "ts": 2_500,
            "event": "phase.complete",
            "phase": "indexer",
            "success": 1
        });
        let row_missing = json!({
            "ts": 2_600,
            "event": "phase.complete",
            "phase": "indexer"
        });
        write_run_log(
            root,
            day1,
            "001.jsonl",
            &[
                &row_failed.to_string(),
                &row_skipped.to_string(),
                &row_int.to_string(),
                &row_missing.to_string(),
            ],
        );

        let view1 = read_backlog_view(
            &FilesystemHealthLogSource::new(root),
            &FilesystemSegmentSource,
            root,
            2,
            now,
        )
        .unwrap();

        // Day states match control journal
        assert_eq!(view1.days[0].state, view_control.days[0].state);
        assert_eq!(view1.days[1].state, view_control.days[1].state);

        let winner1 = view1
            .indexer_phase
            .expect("indexer phase winner must be present");
        assert!(!winner1.success);
        assert_eq!(winner1.run_started_at_ms, 1_000);
        assert_eq!(winner1.reason_code.as_deref(), Some("failed"));

        // 2. A later success: true replaces it
        let row_success = json!({
            "ts": 3_000,
            "event": "phase.complete",
            "phase": "indexer",
            "success": true
        });
        write_run_log(root, day2, "001.jsonl", &[&row_success.to_string()]);

        let view2 = read_backlog_view(
            &FilesystemHealthLogSource::new(root),
            &FilesystemSegmentSource,
            root,
            2,
            now,
        )
        .unwrap();

        assert_eq!(view2.days[0].state, view_control.days[0].state);
        assert_eq!(view2.days[1].state, view_control.days[1].state);

        let winner2 = view2
            .indexer_phase
            .expect("indexer phase winner must be replaced by success");
        assert!(winner2.success);
        assert_eq!(winner2.run_started_at_ms, 3_000);
        assert!(winner2.reason_code.is_none());
    }
}
