// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process health maintenance routines backed by the retention library.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::NaiveDate;
use serde_json::{Map, Value};
use solstone_core_journal_config::read_journal_config;
use solstone_core_retention::content::{ClosedHandlerSet, JournalMedia};
use solstone_core_retention::door::{compact_log, remove_logs, remove_planned_oplogs};
use solstone_core_retention::logs::{
    Compaction, EntryKind, Kept, LogPlan, LogPolicy, day_key, plan as plan_logs, plan_compactions,
};
use solstone_core_retention::marks::{Proposal, RemovalClass, load, reconcile};
use solstone_core_retention::oplog_retention::{OplogRetentionPlan, plan_oplog_retention};
use solstone_core_retention::policy::{policy_from_retention, policy_would_release};
use solstone_core_retention::receipt::Outcome;
use solstone_core_retention::sweep::plan as plan_sweep;

use crate::timezone::host_local_date;
use crate::{CliRun, HealthServices};

const LOG_ENABLED_ERROR: &str = "retention.journal_logs.enabled must be a boolean";
const LOG_DAYS_ERROR: &str = "retention.journal_logs.days must be a positive integer";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LogRetentionConfig {
    enabled: bool,
    days: u32,
}

#[derive(Default)]
struct PruneStats {
    files: u64,
    dirs: u64,
    bytes: u64,
}

struct PruneError {
    reason: String,
    message: String,
    hint: Option<String>,
}

pub(crate) fn run(
    id: &str,
    args: &[String],
    journal: &Path,
    services: &HealthServices<'_>,
) -> CliRun {
    match id {
        "health:mark-raw" if args.is_empty() => mark_raw(journal, services),
        "health:mark-raw" => usage_error("health:mark-raw", args, ""),
        "health:prune-logs" => prune_logs(args, journal, services),
        _ => usage_error(id, args, ""),
    }
}

fn mark_raw(journal: &Path, services: &HealthServices<'_>) -> CliRun {
    let config = match read_config(journal) {
        Ok(config) => config,
        Err(error) => return mark_unavailable(error),
    };
    let retention = config
        .get("retention")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let policy = policy_from_retention(&retention);
    if !policy_would_release(&policy) {
        return success("mark-raw: your retention settings keep all original media.".to_owned());
    }

    // Python uses no-argument `datetime.astimezone()` here: this is host-local,
    // not the configured owner timezone used by timeline rollups.
    let today = host_local_date(services.now, services.host_timezone);
    let before = match load(journal) {
        Ok(register) => register,
        Err(_) => return mark_refused(),
    };
    let plan = plan_sweep(
        journal,
        &policy,
        &ClosedHandlerSet,
        &JournalMedia,
        today,
        services.now,
    );
    if plan.chronicle_unavailable {
        return mark_refused();
    }
    let mut proposals = plan
        .candidates
        .iter()
        .map(|candidate| {
            let mut names = candidate
                .proven
                .iter()
                .map(|item| item.name().to_owned())
                .collect::<Vec<_>>();
            names.sort();
            (
                candidate.target.clone(),
                Proposal {
                    bytes: candidate.bytes(),
                    reason: format!("policy eligibility: {:?}", candidate.eligibility),
                    names,
                },
            )
        })
        .collect::<Vec<_>>();
    if !plan.unreadable_days.is_empty() {
        proposals.extend(
            before
                .marks
                .values()
                .filter(|mark| {
                    mark.class == RemovalClass::PolicyRawRelease
                        && plan.unreadable_days.contains(&mark.target.day)
                })
                .map(|mark| (mark.target.clone(), mark.proposal.clone())),
        );
    }
    let after = match reconcile(
        journal,
        RemovalClass::PolicyRawRelease,
        &proposals,
        services.now,
    ) {
        Ok(register) => register,
        Err(_) => return mark_refused(),
    };
    let before_ids = policy_mark_ids(&before);
    let after_ids = policy_mark_ids(&after);
    let new_ids = after_ids
        .difference(&before_ids)
        .cloned()
        .collect::<Vec<_>>();
    let mut output = format!(
        "mark-raw: new items: {}.\n  standing total: {}.\n",
        new_ids.len(),
        after_ids.len()
    );
    for id in new_ids {
        if let Some(mark) = after.marks.get(&id) {
            output.push_str(&format!("  {}: {}\n", id.as_str(), mark.proposal.reason));
        }
    }
    success(output.trim_end().to_owned())
}

fn prune_logs(args: &[String], journal: &Path, services: &HealthServices<'_>) -> CliRun {
    let (dry_run, override_days) = match parse_prune_args(args) {
        Ok(parsed) => parsed,
        Err(PruneArgError::Usage(message)) => {
            return usage_error("health:prune-logs", args, &message);
        }
        Err(PruneArgError::InvalidDays) => {
            return CliRun {
                stdout: String::new(),
                stderr: "prune-logs: --days must be a positive integer\n".to_owned(),
                exit_code: 1,
            };
        }
    };
    let config = match load_log_retention_config(journal) {
        Ok(config) => config,
        Err(error) => return prune_unavailable(error),
    };
    let days = match override_days {
        Some(days) => days,
        None => config.days,
    };
    if !config.enabled {
        return success("prune-logs: disabled".to_owned());
    }
    if !journal.is_dir() {
        return prune_unavailable("the journal directory is unavailable".to_owned());
    }
    // Python's `date.today()` is host-local for this routine, deliberately not
    // owner-local. The injected host seam keeps the result deterministic in tests.
    let today = host_local_date(services.now, services.host_timezone);
    let policy = LogPolicy {
        days,
        enabled: true,
    };
    let plan = plan_logs(journal, &policy, today);
    let oplogs = plan_oplog_retention(journal, &policy, today);
    let compactions = plan_compactions(journal, &policy, today, &plan);
    if dry_run {
        return render_prune(today, days, true, &plan, &oplogs, &compactions);
    }
    let mut outcome = remove_logs(journal, &plan.prunable);
    let oplog_outcome = remove_planned_oplogs(journal, &oplogs.prunable);
    outcome.targets.extend(oplog_outcome.targets);
    if outcome.halted.is_none() {
        outcome.halted = oplog_outcome.halted;
    }
    for compaction in &compactions {
        let compacted = compact_log(journal, compaction);
        outcome.targets.extend(compacted.targets);
        if outcome.halted.is_none() {
            outcome.halted = compacted.halted;
        }
    }
    if outcome.halted.is_some()
        || outcome
            .targets
            .iter()
            .any(|target| !target.not_removed.is_empty())
    {
        return prune_refused(&outcome);
    }
    render_prune(today, days, false, &plan, &oplogs, &compactions)
}

fn read_config(journal: &Path) -> Result<Map<String, Value>, String> {
    read_journal_config(journal)
        .map_err(|error| error.to_string())
        .map(|read| read.config.unwrap_or_default())
}

fn load_log_retention_config(journal: &Path) -> Result<LogRetentionConfig, String> {
    let config = read_config(journal)?;
    let retention = config
        .get("retention")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let logs = retention
        .get("journal_logs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let enabled = logs.get("enabled").cloned().unwrap_or(Value::Bool(true));
    let Value::Bool(enabled) = enabled else {
        return Err(LOG_ENABLED_ERROR.to_owned());
    };
    let days = logs.get("days").cloned().unwrap_or(Value::from(30));
    let days = positive_config_days(&days).ok_or_else(|| LOG_DAYS_ERROR.to_owned())?;
    Ok(LogRetentionConfig { enabled, days })
}

fn positive_config_days(value: &Value) -> Option<u32> {
    match value {
        Value::Bool(_) => None,
        Value::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        _ => None,
    }
    .filter(|days| *days >= 1)
}

fn policy_mark_ids(
    register: &solstone_core_retention::marks::Register,
) -> BTreeSet<solstone_core_retention::marks::MarkId> {
    register
        .marks
        .iter()
        .filter(|(_, mark)| mark.class == RemovalClass::PolicyRawRelease)
        .map(|(id, _)| id.clone())
        .collect()
}

enum PruneArgError {
    Usage(String),
    InvalidDays,
}

fn parse_prune_args(args: &[String]) -> Result<(bool, Option<u32>), PruneArgError> {
    let mut dry_run = false;
    let mut days = None;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--dry-run" => dry_run = true,
            "--days" => {
                let Some(value) = args.get(index.saturating_add(1)) else {
                    return Err(PruneArgError::Usage(
                        "argument --days: expected one argument".to_owned(),
                    ));
                };
                let Ok(parsed) = value.parse::<i64>() else {
                    return Err(PruneArgError::Usage(format!(
                        "argument --days: invalid int value: '{value}'"
                    )));
                };
                if parsed < 1 {
                    return Err(PruneArgError::InvalidDays);
                }
                let Ok(parsed) = u32::try_from(parsed) else {
                    return Err(PruneArgError::InvalidDays);
                };
                days = Some(parsed);
                index = index.saturating_add(1);
            }
            _ => {
                return Err(PruneArgError::Usage(format!(
                    "unrecognized arguments: {}",
                    args[index]
                )));
            }
        }
        index = index.saturating_add(1);
    }
    Ok((dry_run, days))
}

fn render_prune(
    today: NaiveDate,
    days: u32,
    dry_run: bool,
    plan: &LogPlan,
    oplogs: &OplogRetentionPlan,
    compactions: &[Compaction],
) -> CliRun {
    let mut stats = BTreeMap::<String, PruneStats>::new();
    let mut errors = Vec::new();
    let mut total_files = 0u64;
    let mut total_dirs = 0u64;
    let mut total_bytes = 0u64;
    for target in &plan.prunable {
        let entry = stats.entry(target.class().to_owned()).or_default();
        match target.kind() {
            EntryKind::File => entry.files = entry.files.saturating_add(1),
            EntryKind::Directory => entry.dirs = entry.dirs.saturating_add(1),
        }
        entry.bytes = entry.bytes.saturating_add(target.bytes());
        total_bytes = total_bytes.saturating_add(target.bytes());
    }
    for target in &oplogs.prunable {
        let entry = stats.entry("oplog_retention".to_owned()).or_default();
        entry.files = entry.files.saturating_add(1);
        entry.bytes = entry.bytes.saturating_add(target.bytes());
        total_bytes = total_bytes.saturating_add(target.bytes());
    }
    for entry in stats.values() {
        total_files = total_files.saturating_add(entry.files);
        total_dirs = total_dirs.saturating_add(entry.dirs);
    }
    for retained in &plan.retained {
        let reason = match &retained.reason {
            Kept::Undateable => {
                Some("the entry's retention date could not be determined".to_owned())
            }
            Kept::ContentMalformed { detail, .. } => {
                Some(format!("malformed talent day-index row: {detail}"))
            }
            Kept::TooYoung(_) | Kept::Exempt | Kept::NotAMatch | Kept::ContentNotFullyOld => None,
        };
        if let Some(reason) = reason {
            errors.push(PruneError {
                message: reason.clone(),
                reason,
                hint: None,
            });
        }
    }
    for compaction in compactions {
        let freed = compaction
            .bytes_before
            .saturating_sub(compaction.bytes_after);
        total_bytes = total_bytes.saturating_add(freed);
    }
    let action = if dry_run { "would prune" } else { "pruned" };
    let cutoff = today
        .checked_sub_days(chrono::Days::new(u64::from(days)))
        .map(day_key)
        .unwrap_or_default();
    let mut output = format!(
        "prune-logs: {action} {total_files} operational-log file(s), {total_dirs} cache dir(s), {} cutoff={cutoff}\n",
        human_bytes(total_bytes)
    );
    for (class, entry) in stats {
        if entry.files > 0 || entry.dirs > 0 || entry.bytes > 0 {
            output.push_str(&format!(
                "  {class}: {} file(s), {} cache dir(s), {}\n",
                entry.files,
                entry.dirs,
                human_bytes(entry.bytes)
            ));
        }
    }
    for error in errors {
        let hint = error
            .hint
            .as_deref()
            .map(|hint| format!(" hint={hint}"))
            .unwrap_or_default();
        output.push_str(&format!(
            "  error: {}: {}{hint}\n",
            error.reason, error.message
        ));
    }
    success(output.trim_end().to_owned())
}

fn human_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if value.abs() < 1024.0 {
            return if unit == "B" {
                format!("{} B", value as u64)
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1024.0;
    }
    format!("{value:.1} PB")
}

fn mark_refused() -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: "mark-raw: some items could not be listed:\n".to_owned(),
        exit_code: 1,
    }
}

fn mark_unavailable(error: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("mark-raw: could not build the list: {error}\n"),
        exit_code: 1,
    }
}

fn prune_unavailable(error: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("prune-logs: could not prune logs: {error}\n"),
        exit_code: 0,
    }
}

fn prune_refused(outcome: &Outcome) -> CliRun {
    let summary = outcome
        .targets
        .iter()
        .flat_map(|target| target.not_removed.iter())
        .map(|failure| format!("{}: {}", failure.entry, failure.reason))
        .collect::<Vec<_>>()
        .join("; ");
    let summary = if summary.is_empty() {
        "the retention tool refused without naming an entry."
    } else {
        &summary
    };
    CliRun {
        stdout: String::new(),
        stderr: format!("prune-logs: some logs could not be pruned: {summary}\n"),
        exit_code: 0,
    }
}

fn success(line: String) -> CliRun {
    CliRun {
        stdout: format!("{line}\n"),
        stderr: String::new(),
        exit_code: 0,
    }
}

fn usage_error(id: &str, args: &[String], detail: &str) -> CliRun {
    let usage = match id {
        "health:prune-logs" => " [-h] [--dry-run] [--days DAYS]",
        _ => " [-h]",
    };
    let detail = if detail.is_empty() {
        format!("unrecognized arguments: {}", args.join(" "))
    } else {
        detail.to_owned()
    };
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "usage: journal maintenance run {id}{usage}\njournal maintenance run {id}: error: {detail}\n"
        ),
        exit_code: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        policy_from_retention, policy_would_release, positive_config_days, prune_refused, run,
    };
    use crate::HealthServices;
    use crate::timezone::HostTimezoneSource;
    use chrono::{FixedOffset, TimeZone, Utc};
    use serde_json::{Map, Value, json};
    use solstone_core_journal_io::JournalRoot;
    use solstone_core_journal_io::operational_log::{OplogFormat, create_oplog_at};
    use solstone_core_retention::Target;
    use solstone_core_retention::marks::load;
    use solstone_core_retention::receipt::{NotRemoved, Outcome, RunHalt, TargetOutcome};

    struct Host;

    impl HostTimezoneSource for Host {
        fn usable_iana_key(&self) -> Option<String> {
            Some("UTC".to_owned())
        }
    }

    struct NamedHost(&'static str);

    impl HostTimezoneSource for NamedHost {
        fn usable_iana_key(&self) -> Option<String> {
            Some(self.0.to_owned())
        }
    }

    #[test]
    fn policy_translation_matches_keep_days_processed_and_stream_overrides() {
        let retention = object(json!({
            "raw_media": "days", "raw_media_days": 3,
            "raw_media_minimum_days": -7,
            "per_stream": {
                "keep": {"raw_media": "keep"},
                "processed": {"raw_media": "processed"},
                "bad": 1
            }
        }));
        let policy = policy_from_retention(&retention);
        assert_eq!(policy.default_rule.period.unwrap().0, 3);
        assert!(policy.rule_for("keep").period.is_none());
        assert_eq!(policy.rule_for("processed").period.unwrap().0, 0);
        assert_eq!(policy.minimum_age.0, 0);
        assert!(policy_would_release(&policy));
    }

    #[test]
    fn invalid_days_keep_and_config_days_reject_bool_and_nonpositive() {
        let policy = policy_from_retention(&object(
            json!({"raw_media": "days", "raw_media_days": 0, "empty_audio": "keep"}),
        ));
        assert!(!policy_would_release(&policy));
        assert_eq!(positive_config_days(&json!(30)), Some(30));
        assert_eq!(positive_config_days(&json!(true)), None);
        assert_eq!(positive_config_days(&json!(0)), None);
    }

    #[test]
    fn keep_journal_with_no_empty_audio_does_not_create_a_mark_register() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("config")).unwrap();
        std::fs::create_dir_all(journal.path().join("chronicle")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            b"{\"retention\": {\"raw_media\": \"keep\"}}",
        )
        .unwrap();
        let host = Host;
        let result = run(
            "health:mark-raw",
            &[],
            journal.path(),
            &HealthServices {
                now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 0, 0).unwrap(),
                host_timezone: &host,
            },
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("new items: 0"));
        assert!(result.stdout.contains("standing total: 0"));
        assert!(!journal.path().join("health/retention-marks.json").exists());
    }

    #[test]
    fn mark_raw_uses_host_date_even_when_owner_timezone_is_configured() {
        // Timeline rollups use `identity.timezone`; this health routine matches
        // Python's no-argument `astimezone()` and must deliberately ignore it.
        let journal = tempfile::tempdir().unwrap();
        write_config(
            journal.path(),
            json!({
                "identity": {"timezone": "Asia/Tokyo"},
                "retention": {"raw_media": "days", "raw_media_days": 1}
            }),
        );
        write_proven_segment(journal.path(), "20260301", "field.audio", "070000_17");
        let host = NamedHost("America/Los_Angeles");
        let result = run(
            "health:mark-raw",
            &[],
            journal.path(),
            &HealthServices {
                // This is Mar 1 in Los Angeles and Mar 2 in configured Tokyo.
                now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 30, 0).unwrap(),
                host_timezone: &host,
            },
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.stdout,
            "mark-raw: new items: 0.\n  standing total: 0.\n"
        );
    }

    #[test]
    fn mark_raw_reports_only_new_marks_and_keeps_the_full_standing_total() {
        let journal = tempfile::tempdir().unwrap();
        write_config(
            journal.path(),
            json!({"retention": {"raw_media": "days", "raw_media_days": 7}}),
        );
        write_proven_segment(journal.path(), "20260130", "first.audio", "070000_17");
        let host = Host;
        let services = HealthServices {
            now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 0, 0).unwrap(),
            host_timezone: &host,
        };
        let first = run("health:mark-raw", &[], journal.path(), &services);
        assert_eq!(first.exit_code, 0);
        assert!(
            first
                .stdout
                .starts_with("mark-raw: new items: 1.\n  standing total: 1.")
        );
        assert_eq!(
            load(journal.path())
                .unwrap()
                .marks
                .values()
                .next()
                .unwrap()
                .proposal
                .reason,
            "policy eligibility: Eligible { anchor: Captured, age_days: 30, period: Days(7) }"
        );
        let first_id = mark_ids_from_output(&first.stdout).pop().unwrap();

        // A second stream becomes eligible after the first register reconciliation.
        write_proven_segment(journal.path(), "20260130", "second.audio", "070000_17");
        let second = run("health:mark-raw", &[], journal.path(), &services);
        assert_eq!(second.exit_code, 0);
        assert!(
            second
                .stdout
                .starts_with("mark-raw: new items: 1.\n  standing total: 2.")
        );
        let second_ids = mark_ids_from_output(&second.stdout);
        assert_eq!(second_ids.len(), 1);
        assert_ne!(second_ids[0], first_id);
    }

    #[test]
    fn prune_argument_validation_matches_manual_and_argparse_branches() {
        let journal = tempfile::tempdir().unwrap();
        let host = Host;
        let services = HealthServices {
            now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 0, 0).unwrap(),
            host_timezone: &host,
        };
        let zero = run(
            "health:prune-logs",
            &["--days".to_owned(), "0".to_owned()],
            journal.path(),
            &services,
        );
        assert_eq!(zero.exit_code, 1);
        assert_eq!(
            zero.stderr,
            "prune-logs: --days must be a positive integer\n"
        );
        let text = run(
            "health:prune-logs",
            &["--days".to_owned(), "nope".to_owned()],
            journal.path(),
            &services,
        );
        assert_eq!(text.exit_code, 2);
        assert!(
            text.stderr
                .starts_with("usage: journal maintenance run health:prune-logs")
        );
    }

    #[test]
    fn disabled_prune_short_circuits_before_any_retention_work() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            b"{\"retention\": {\"journal_logs\": {\"enabled\": false}}}",
        )
        .unwrap();
        let host = Host;
        let result = run(
            "health:prune-logs",
            &[],
            journal.path(),
            &HealthServices {
                now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 0, 0).unwrap(),
                host_timezone: &host,
            },
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "prune-logs: disabled\n");
        assert!(!journal.path().join("health").exists());
    }

    #[test]
    fn prune_dry_run_only_plans_and_execute_uses_the_retention_door() {
        let journal = tempfile::tempdir().unwrap();
        let token = journal.path().join("tokens/20200101.jsonl");
        std::fs::create_dir_all(token.parent().unwrap()).unwrap();
        std::fs::write(&token, b"old token log\n").unwrap();
        let host = Host;
        let services = HealthServices {
            now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 0, 0).unwrap(),
            host_timezone: &host,
        };
        let dry_run = run(
            "health:prune-logs",
            &["--dry-run".to_owned()],
            journal.path(),
            &services,
        );
        assert_eq!(dry_run.exit_code, 0);
        assert!(
            dry_run
                .stdout
                .starts_with("prune-logs: would prune 1 operational-log file(s)")
        );
        assert!(token.exists());

        let executed = run("health:prune-logs", &[], journal.path(), &services);
        assert_eq!(executed.exit_code, 0);
        assert!(
            executed
                .stdout
                .starts_with("prune-logs: pruned 1 operational-log file(s)")
        );
        assert!(!token.exists());
    }

    #[test]
    fn prune_logs_plans_and_removes_released_canonical_oplogs() {
        let journal = tempfile::tempdir().unwrap();
        let instant = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 1, 12, 0, 0)
            .single()
            .unwrap();
        let leaf = {
            let writer = create_oplog_at(
                JournalRoot::open(journal.path()).unwrap(),
                "source",
                "run",
                OplogFormat::Log,
                instant,
            )
            .unwrap();
            writer.leaf_name().to_owned()
        };
        let path = journal.path().join("chronicle/20260101/health").join(&leaf);
        let host = Host;
        let services = HealthServices {
            now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 0, 0).unwrap(),
            host_timezone: &host,
        };

        let dry_run = run(
            "health:prune-logs",
            &["--dry-run".to_owned()],
            journal.path(),
            &services,
        );
        assert_eq!(dry_run.exit_code, 0);
        assert!(
            dry_run
                .stdout
                .contains("would prune 1 operational-log file(s)")
        );
        assert!(dry_run.stdout.contains("oplog_retention: 1 file(s)"));
        assert!(path.exists(), "dry-run keeps the planned oplog");

        let executed = run("health:prune-logs", &[], journal.path(), &services);
        assert_eq!(executed.exit_code, 0);
        assert!(executed.stdout.contains("pruned 1 operational-log file(s)"));
        assert!(executed.stdout.contains("oplog_retention: 1 file(s)"));
        assert!(
            !path.exists(),
            "the daily health routine removed the planned oplog"
        );
    }

    #[test]
    fn prune_logs_refusal_uses_the_reference_stderr_without_a_breakdown() {
        let outcome = Outcome {
            targets: vec![TargetOutcome {
                target: Target {
                    day: String::new(),
                    stream: String::new(),
                    dir: "tokens".to_owned(),
                },
                removed: Vec::new(),
                not_removed: vec![NotRemoved {
                    entry: "tokens/20260101.jsonl".to_owned(),
                    reason: "the log entry could not be removed: permission denied".to_owned(),
                    staged: None,
                }],
                post_commit_failure: None,
            }],
            halted: None,
        };
        let result = prune_refused(&outcome);
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.is_empty());
        assert_eq!(
            result.stderr,
            "prune-logs: some logs could not be pruned: tokens/20260101.jsonl: the log entry could not be removed: permission denied\n"
        );
        assert!(!result.stderr.contains("cutoff="));
        assert!(!result.stderr.contains("pruned "));
    }

    #[test]
    fn prune_logs_halted_without_named_entries_uses_the_reference_fallback() {
        let outcome = Outcome {
            targets: Vec::new(),
            halted: Some(RunHalt {
                reason: "fixture halt".to_owned(),
            }),
        };
        let result = prune_refused(&outcome);
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.is_empty());
        assert_eq!(
            result.stderr,
            "prune-logs: some logs could not be pruned: the retention tool refused without naming an entry.\n"
        );
    }

    fn write_config(journal: &std::path::Path, config: Value) {
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(journal.join("config/journal.json"), config.to_string()).unwrap();
    }

    fn write_proven_segment(journal: &std::path::Path, day: &str, stream: &str, dir: &str) {
        let segment = journal.join("chronicle").join(day).join(stream).join(dir);
        std::fs::create_dir_all(&segment).unwrap();
        let raw = b"the owner's recording";
        std::fs::write(segment.join("audio.flac"), raw).unwrap();
        let header = json!({
            "segment": dir,
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "analyzed",
                "reason_code": "ok",
                "handler": "transcribe",
                "attempted_at": "2026-02-28T00:00:00Z",
                "input_size": raw.len(),
            }
        });
        std::fs::write(
            segment.join("audio.jsonl"),
            format!("{header}\n{{\"start\":0.0,\"text\":\"hello\"}}\n"),
        )
        .unwrap();
    }

    fn mark_ids_from_output(output: &str) -> Vec<&str> {
        output
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .filter_map(|line| line.split_once(": policy eligibility:"))
            .map(|(id, _)| id)
            .collect()
    }

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }
}
