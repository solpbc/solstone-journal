// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Names every piece of bookkeeping the journal healed rather than refused:
//! artifacts set aside as `<name>.wedged-<stamp>`, lifecycle generations a
//! successor closed because both their authorities died, and the set-aside
//! `health/parent-loss.wedged-*` trees an operator or an older heal left.
//! Nothing here is a fault to fix; it is the visible half of a heal.

use solstone_core_cli_copy as copy;

use std::fs;
use std::path::Path;

use crate::context::CheckContext;
use crate::vocabulary::{Check, ReportForm, RunnerResult, Status, make_result, truncate};

use solstone_core_journal_io::durability::SET_ASIDE_MARKER;

/// The doctor names at most this many items per kind; the rest are counted.
const NAMED_LIMIT: usize = 8;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct DurabilityReport {
    /// Relative paths of set-aside artifacts, under `health/` and `config/`.
    pub set_aside: Vec<String>,
    /// One line per generation a successor closed.
    pub closed_generations: Vec<String>,
    /// Generations sealed `unresolved` by their own coordinator.
    pub unresolved_generations: Vec<String>,
    /// Legacy English entries returned in the structured heals array.
    canonical_heals: Vec<String>,
    canonical_unresolved: Vec<String>,
    /// What could not be inspected.
    pub unreadable: Vec<String>,
    /// Generations a start refuses on: a retirement's descendant record lost.
    pub refusals: Vec<String>,
    /// Launches the newest generation still owes, when it has a directory.
    pub outstanding: Option<(u64, usize)>,
}

impl DurabilityReport {
    fn is_clean(&self) -> bool {
        self.set_aside.is_empty()
            && self.closed_generations.is_empty()
            && self.unresolved_generations.is_empty()
            && self.unreadable.is_empty()
            && self.refusals.is_empty()
    }
}

/// A journal-relative name reads the same on every platform: `/`-separated,
/// as the journal's own records name their paths.
fn relative(journal: &Path, path: &Path) -> String {
    match path.strip_prefix(journal) {
        Ok(inside) => inside
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    }
}

/// Collect set-aside names by traversing directories dynamically.
fn scan_directory_recursive(journal: &Path, directory: &Path, report: &mut DurabilityReport) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            report
                .unreadable
                .push(format!("{}: {error}", relative(journal, directory)));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report
                    .unreadable
                    .push(format!("{}: {error}", relative(journal, directory)));
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name();
        // Discharged entries and folded caches wait there for the prune.
        if name == "discharged" {
            continue;
        }
        if name.to_string_lossy().contains(SET_ASIDE_MARKER) {
            report.set_aside.push(relative(journal, &path));
        }
        let is_dir = match entry.file_type() {
            Ok(ft) => ft.is_dir(),
            Err(error) => {
                report
                    .unreadable
                    .push(format!("{}: {error}", relative(journal, &path)));
                false
            }
        };
        if is_dir {
            scan_directory_recursive(journal, &path, report);
        }
    }
}

#[cfg(unix)]
fn scan_generations(journal: &Path, report: &mut DurabilityReport, form: ReportForm) {
    use solstone_core_system::lifecycle::{
        AdmissionFinding, ParentLossGenerationRecord, ParentLossTerminalDisposition,
        parent_loss_census_is_lost,
    };

    let generations = journal.join("health/parent-loss/generations");
    let entries = match fs::read_dir(&generations) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            report
                .unreadable
                .push(format!("{}: {error}", relative(journal, &generations)));
            return;
        }
    };
    let mut numbered = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report
                    .unreadable
                    .push(format!("{}: {error}", relative(journal, &generations)));
                continue;
            }
        };
        if let Some(generation) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u64>().ok())
        {
            numbered.push((generation, entry.path()));
        }
    }
    numbered.sort();
    if let Some((generation, directory)) = numbered.last() {
        report.outstanding = Some((*generation, count_entries(&directory.join("admissions"))));
    }
    for (generation, directory) in numbered {
        let record_path = directory.join("record.json");
        let bytes = match fs::read(&record_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                report
                    .unreadable
                    .push(format!("{}: {error}", relative(journal, &record_path)));
                continue;
            }
        };
        let record =
            match bytes.map(|bytes| serde_json::from_slice::<ParentLossGenerationRecord>(&bytes)) {
                None => None,
                Some(Ok(record)) => Some(record),
                Some(Err(error)) => {
                    report
                        .unreadable
                        .push(format!("{}: {error}", relative(journal, &record_path)));
                    continue;
                }
            };
        let closed = record
            .as_ref()
            .is_some_and(|record| record.closure.is_some());
        if !closed && parent_loss_census_is_lost(&directory) {
            // A Mac generation from before its boot time was recorded can
            // never prove a restart, so it cannot clear on its own.
            let boot_time_recorded = fs::read(directory.join("boot.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|boot| boot.get("boot_time").is_some_and(|time| time.is_u64()));
            let detail = if cfg!(target_os = "macos") && !boot_time_recorded {
                copy::display(
                    "journal.cli.doctor.durability.refusal.support",
                    [("generation", &generation)],
                )
            } else {
                copy::display(
                    "journal.cli.doctor.durability.refusal.restart",
                    [("generation", &generation)],
                )
            };
            report.refusals.push(detail);
        }
        let Some(record) = record else {
            continue;
        };
        if let Some(closure) = record.closure.as_ref() {
            let (total, exited, retired) = match closure.summary.as_ref() {
                Some(summary) => (
                    summary.total() as usize,
                    summary.exited as usize,
                    summary.retired as usize,
                ),
                None => (
                    closure.admissions.len(),
                    closure
                        .admissions
                        .iter()
                        .filter(|admission| admission.finding == AdmissionFinding::Exited)
                        .count(),
                    closure
                        .admissions
                        .iter()
                        .filter(|admission| {
                            matches!(admission.finding, AdmissionFinding::Retired { .. })
                        })
                        .count(),
                ),
            };
            let never_admitted = total.saturating_sub(exited).saturating_sub(retired);
            let closed = |language: ReportForm| {
                if closure
                    .summary
                    .as_ref()
                    .is_some_and(|summary| summary.boot_changed)
                {
                    language.counted(
                        "journal.cli.doctor.durability.closed.restarted",
                        &[("quantity", total as u64)],
                        &[
                            ("generation", &generation),
                            ("successor", &closure.successor_generation),
                            ("time", &closure.closed_at),
                            ("total", &total),
                            ("exited", &exited),
                            ("retired", &retired),
                            ("never", &never_admitted),
                        ],
                    )
                } else {
                    language.counted(
                        "journal.cli.doctor.durability.closed",
                        &[("quantity", total as u64)],
                        &[
                            ("generation", &generation),
                            ("successor", &closure.successor_generation),
                            ("time", &closure.closed_at),
                            ("total", &total),
                            ("exited", &exited),
                            ("retired", &retired),
                            ("never", &never_admitted),
                        ],
                    )
                }
            };
            report.closed_generations.push(closed(form));
            report
                .canonical_heals
                .push(closed(ReportForm::CanonicalEnglish));
            for admission in &closure.admissions {
                let finding = match admission.finding {
                    AdmissionFinding::Exited => "exited",
                    AdmissionFinding::Retired { escalated: true } => "escalated",
                    AdmissionFinding::Retired { escalated: false } => "retired",
                    AdmissionFinding::NeverAcknowledged => "unacknowledged",
                    AdmissionFinding::SpawnFailed => "spawn",
                    AdmissionFinding::Unidentified => "unidentified",
                    AdmissionFinding::RejectedAndReaped => "rejected",
                    AdmissionFinding::PidReusedByAnotherUser => "reused",
                };
                let row = |language: ReportForm| match admission.service {
                    Some(service) => {
                        let identity = solstone_core_system::lifecycle::service_name(service);
                        copy::display_for(
                            language.locale(),
                            "journal.cli.doctor.durability.admission.service",
                            [
                                ("finding", &finding),
                                ("generation", &generation),
                                ("admission", &admission.launch_id),
                                ("identity", &identity),
                            ],
                        )
                    }
                    None => {
                        let identity = admission.launch_id.split('-').next().unwrap_or("helper");
                        copy::display_for(
                            language.locale(),
                            "journal.cli.doctor.durability.admission.helper",
                            [
                                ("finding", &finding),
                                ("generation", &generation),
                                ("admission", &admission.launch_id),
                                ("identity", &identity),
                            ],
                        )
                    }
                };
                report.closed_generations.push(row(form));
                report
                    .canonical_heals
                    .push(row(ReportForm::CanonicalEnglish));
            }
        } else if let Some(ParentLossTerminalDisposition::Unresolved { reason }) =
            record.terminal.as_ref()
        {
            let row = |language: ReportForm| {
                copy::display_for(
                    language.locale(),
                    "journal.cli.doctor.durability.unresolved",
                    [("generation", &generation), ("reason", &reason)],
                )
            };
            report.unresolved_generations.push(row(form));
            report
                .canonical_unresolved
                .push(row(ReportForm::CanonicalEnglish));
        }
    }
}

#[cfg(not(unix))]
fn scan_generations(_journal: &Path, _report: &mut DurabilityReport, _form: ReportForm) {}

/// Entry directories in one `admissions/` listing.
#[cfg(unix)]
fn count_entries(admissions: &Path) -> usize {
    fs::read_dir(admissions)
        .map(|children| {
            children
                .flatten()
                .filter(|child| child.file_type().is_ok_and(|kind| kind.is_dir()))
                .count()
        })
        .unwrap_or(0)
}

pub(crate) fn scan(journal: &Path, form: ReportForm) -> DurabilityReport {
    let mut report = DurabilityReport::default();
    let mut root_prefixes = std::collections::BTreeSet::new();
    for entry in solstone_core_journal_io::durability::artifacts() {
        if let Some(first) = entry
            .path
            .split(['/', '\\'])
            .next()
            .filter(|f| !f.is_empty() && !f.contains('*'))
        {
            root_prefixes.insert(first.to_owned());
        }
    }
    for prefix in root_prefixes {
        let dir = journal.join(prefix);
        scan_directory_recursive(journal, &dir, &mut report);
    }
    scan_generations(journal, &mut report, form);
    report.set_aside.sort();
    report.set_aside.dedup();
    report
}

fn named(id: &str, items: &[String]) -> String {
    let mut lines = vec![copy::display(id, [("count", &items.len())])];
    for item in items.iter().take(NAMED_LIMIT) {
        lines.push(format!("  {item}"));
    }
    if items.len() > NAMED_LIMIT {
        lines.push(format!(
            "  {}",
            copy::display(
                "journal.cli.doctor.durability.more",
                [("count", &(items.len() - NAMED_LIMIT))]
            )
        ));
    }
    lines.join("\n")
}

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.journal.absent"),
            None::<String>,
        ));
    }
    let report = scan(&context.journal_path, context.report_form);
    if report.is_clean() {
        let detail = match report.outstanding {
            Some((generation, count)) => context.report_form.counted(
                "journal.cli.doctor.durability.clean.outstanding",
                &[("quantity", count as u64)],
                &[("generation", &generation), ("launches", &count)],
            ),
            None => copy::text("journal.cli.doctor.durability.clean"),
        };
        return Ok(make_result(check, Status::Ok, detail, None::<String>));
    }
    let mut sections = Vec::new();
    if !report.refusals.is_empty() {
        sections.push(named(
            "journal.cli.doctor.durability.heading.refused",
            &report.refusals,
        ));
    }
    let mut heals = Vec::new();
    if !report.closed_generations.is_empty() {
        sections.push(named(
            "journal.cli.doctor.durability.heading.closed",
            &report.closed_generations,
        ));
        heals.extend(report.canonical_heals);
    }
    if !report.unresolved_generations.is_empty() {
        sections.push(named(
            "journal.cli.doctor.durability.heading.unresolved",
            &report.unresolved_generations,
        ));
        heals.extend(report.canonical_unresolved);
    }
    if !report.set_aside.is_empty() {
        sections.push(named(
            "journal.cli.doctor.durability.heading.aside",
            &report.set_aside,
        ));
        heals.extend(report.set_aside.clone());
    }
    if !report.unreadable.is_empty() {
        sections.push(named(
            "journal.cli.doctor.durability.heading.unreadable",
            &report.unreadable,
        ));
    }
    if let Some((generation, count)) = report.outstanding {
        sections.push(context.report_form.counted(
            "journal.cli.doctor.durability.outstanding",
            &[("quantity", count as u64)],
            &[("generation", &generation), ("launches", &count)],
        ));
    }
    let detail = sections.join("\n");
    let status = if report.unreadable.is_empty() && report.refusals.is_empty() {
        Status::Warn
    } else {
        Status::Fail
    };
    let action = if !report.refusals.is_empty() {
        copy::text("journal.cli.doctor.durability.action.refused")
    } else if report.unreadable.is_empty() {
        copy::text("journal.cli.doctor.durability.action.recovered")
    } else {
        copy::text("journal.cli.doctor.durability.action.unreadable")
    };
    let mut result = make_result(check, status, truncate(&detail, 4096), Some(action));
    if !heals.is_empty() {
        result.heals = Some(heals);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::path::PathBuf;

    use super::*;

    struct TempDir(tempfile::TempDir);

    impl TempDir {
        fn new() -> std::io::Result<Self> {
            tempfile::Builder::new()
                .prefix("solstone-doctor-durability-")
                .tempdir()
                .map(Self)
        }

        fn path(&self) -> &Path {
            self.0.path()
        }
    }

    #[test]
    fn a_journal_without_heals_is_clean() {
        let journal = TempDir::new().expect("journal");
        fs::create_dir_all(journal.path().join("health/parent-loss/generations/1")).unwrap();
        assert!(scan(journal.path(), ReportForm::Structured).is_clean());
    }

    #[test]
    fn set_aside_artifacts_are_named_wherever_the_reader_puts_them() {
        let journal = TempDir::new().expect("journal");
        let health = journal.path().join("health");
        fs::create_dir_all(health.join("parent-loss/generations/3")).unwrap();
        fs::write(health.join("catchup-state.wedged-1700000000.json"), b"x").unwrap();
        fs::write(
            health.join("parent-loss/active-generation.wedged-1700000001.json"),
            b"x",
        )
        .unwrap();
        fs::write(
            health.join("parent-loss/generations/3/record.wedged-1700000002.json"),
            b"x",
        )
        .unwrap();
        fs::create_dir_all(health.join("parent-loss.wedged-20260915-115634")).unwrap();
        let report = scan(journal.path(), ReportForm::Structured);
        assert_eq!(
            report.set_aside,
            vec![
                "health/catchup-state.wedged-1700000000.json".to_owned(),
                "health/parent-loss.wedged-20260915-115634".to_owned(),
                "health/parent-loss/active-generation.wedged-1700000001.json".to_owned(),
                "health/parent-loss/generations/3/record.wedged-1700000002.json".to_owned(),
            ]
        );
        assert!(report.unreadable.is_empty());
    }

    fn test_context(journal_path: std::path::PathBuf) -> CheckContext {
        CheckContext {
            report_form: crate::vocabulary::ReportForm::Structured,
            home_dir: std::path::PathBuf::new(),
            install_bin_dir: std::path::PathBuf::new(),
            journal_path,
            callosum_socket_path: std::path::PathBuf::new(),
            platform: crate::vocabulary::Platform::Linux,
            now: jiff::Timestamp::now(),
            host_arch: String::new(),
            hostname: "host".to_owned(),
            checkout_root: None,
            payload_root: None,
            port: 0,
            service_status_timeout: std::time::Duration::ZERO,
            service_status_command_override: None,
            parakeet_server_probe_override: None,
            speakers_analyze_resolvers: None,
            vad_runtime_probe: None,
            free_space_bytes_override: None,
            windows_service_probe: None,
        }
    }

    fn test_check() -> Check {
        Check {
            name: "journal_durability",
            severity: crate::vocabulary::Severity::Advisory,
            platforms: &[],
        }
    }

    #[test]
    fn nine_or_more_wedged_files_are_all_present_in_heals() {
        let journal = TempDir::new().expect("journal");
        let health = journal.path().join("health");
        fs::create_dir_all(health.join("providers/runtime")).unwrap();
        fs::create_dir_all(health.join("activity-work")).unwrap();

        fs::write(health.join("providers/runtime/local.wedged-1.json"), b"x").unwrap();
        fs::write(health.join("activity-work/dead.wedged-1.json"), b"x").unwrap();

        for i in 1..=8 {
            fs::write(health.join(format!("file-{i}.wedged-1.json")), b"x").unwrap();
        }

        let context = test_context(journal.path().to_path_buf());
        let result = run(&context, test_check()).expect("run");
        assert_eq!(result.status, Status::Warn);
        assert_eq!(
            result.fix.as_deref(),
            Some(
                "no action is needed: recovered records stay beside the originals and are listed here"
            )
        );
        let heals = result.heals.expect("heals list");
        assert_eq!(heals.len(), 10);
        assert!(heals.contains(&"health/providers/runtime/local.wedged-1.json".to_string()));
        assert!(heals.contains(&"health/activity-work/dead.wedged-1.json".to_string()));
        for i in 1..=8 {
            assert!(heals.contains(&format!("health/file-{i}.wedged-1.json")));
        }
        // Human detail caps at 8
        assert!(result.detail.contains("... and 2 more"));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_scan_directory_fails_and_names_path() {
        use std::os::unix::fs::PermissionsExt;

        struct ModeGuard(PathBuf);
        impl Drop for ModeGuard {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
        }

        let journal = TempDir::new().expect("journal");
        let health = journal.path().join("health");
        fs::create_dir_all(&health).unwrap();
        let _guard = ModeGuard(health.clone());
        fs::set_permissions(&health, fs::Permissions::from_mode(0o000)).unwrap();

        let context = test_context(journal.path().to_path_buf());
        let result = run(&context, test_check()).expect("run");

        assert_eq!(result.status, Status::Fail);
        assert_eq!(
            result.fix.as_deref(),
            Some(
                "check the paths under 'could not inspect'; any recovered records stay beside the originals and are listed here"
            )
        );
        assert!(
            result.detail.contains("health: Permission denied")
                || result.detail.contains("health:")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_closed_generation_is_named_with_what_became_of_its_admissions() {
        use solstone_core_system::lifecycle::{
            AbandonedGenerationClosure, AdmissionClosure, AdmissionFinding, AuthorityObservation,
            HeartbeatRetirement, PARENT_LOSS_CLOSURE_SCHEMA_V1, PARENT_LOSS_LEDGER_SCHEMA_V1,
            ParentLossGenerationRecord, ParentLossTerminalDisposition, ParentLossUnresolvedReason,
        };
        use solstone_core_system::process::{ProcessBirth, ProcessInstance};

        let instance = |pid| ProcessInstance {
            pid,
            birth: ProcessBirth::linux(1, 1, 100),
        };
        let journal = TempDir::new().expect("journal");
        let generations = journal.path().join("health/parent-loss/generations");
        fs::create_dir_all(generations.join("93")).unwrap();
        fs::create_dir_all(generations.join("6")).unwrap();
        let closed = ParentLossGenerationRecord {
            schema: PARENT_LOSS_LEDGER_SCHEMA_V1,
            generation: 93,
            coordinator: Some(instance(2)),
            supervisor: instance(1),
            sealed_ledger_digest: Some("d".to_owned()),
            terminal: Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::AuthoritiesLost,
            }),
            supervisor_heartbeat: None,
            closure: Some(AbandonedGenerationClosure {
                schema: PARENT_LOSS_CLOSURE_SCHEMA_V1,
                closed_by: instance(3),
                successor_generation: 94,
                closed_at: 1_789_000_000,
                supervisor: AuthorityObservation::Exited,
                coordinator: AuthorityObservation::Exited,
                heartbeat: HeartbeatRetirement::Removed,
                set_aside: Vec::new(),
                notes: Vec::new(),
                admissions: vec![
                    AdmissionClosure {
                        launch_id: "convey-a".to_owned(),
                        service: None,
                        instance: Some(instance(4)),
                        uid: Some(501),
                        finding: AdmissionFinding::Exited,
                    },
                    AdmissionClosure {
                        launch_id: "sense-b".to_owned(),
                        service: None,
                        instance: Some(instance(5)),
                        uid: Some(501),
                        finding: AdmissionFinding::Retired { escalated: true },
                    },
                    AdmissionClosure {
                        launch_id: "sense-c".to_owned(),
                        service: None,
                        instance: None,
                        uid: None,
                        finding: AdmissionFinding::NeverAcknowledged,
                    },
                ],
                summary: None,
            }),
        };
        fs::write(
            generations.join("93/record.json"),
            serde_json::to_vec(&closed).unwrap(),
        )
        .unwrap();
        let unresolved = ParentLossGenerationRecord {
            schema: PARENT_LOSS_LEDGER_SCHEMA_V1,
            generation: 6,
            coordinator: Some(instance(2)),
            supervisor: instance(1),
            sealed_ledger_digest: Some("d".to_owned()),
            terminal: Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::RetirementDeadlineExceeded {
                    deadline_seconds: 15,
                },
            }),
            supervisor_heartbeat: None,
            closure: None,
        };
        fs::write(
            generations.join("6/record.json"),
            serde_json::to_vec(&unresolved).unwrap(),
        )
        .unwrap();

        let report = scan(journal.path(), ReportForm::Structured);
        assert_eq!(report.closed_generations.len(), 4);
        assert!(report.closed_generations[0].contains("93"));
        assert!(
            report
                .closed_generations
                .iter()
                .any(|line| line.contains("convey-a"))
        );
        assert!(
            report
                .closed_generations
                .iter()
                .any(|line| { line.contains("sense-b") && line.contains("escalated") })
        );
        assert!(
            report
                .closed_generations
                .iter()
                .any(|line| { line.contains("sense-c") })
        );
        assert_eq!(report.unresolved_generations.len(), 1);
        assert!(report.unresolved_generations[0].contains("6"));
        assert!(!report.is_clean());
    }

    #[test]
    fn an_unreadable_record_is_reported_not_fatal() {
        let journal = TempDir::new().expect("journal");
        let generation = journal.path().join("health/parent-loss/generations/2");
        fs::create_dir_all(&generation).unwrap();
        fs::write(generation.join("record.json"), b"{ not json").unwrap();
        let report = scan(journal.path(), ReportForm::Structured);
        #[cfg(unix)]
        assert_eq!(report.unreadable.len(), 1);
        #[cfg(not(unix))]
        assert!(report.unreadable.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_summary_counts_a_closure_and_its_rows_name_only_what_was_noteworthy() {
        use solstone_core_system::lifecycle::{
            AbandonedGenerationClosure, AdmissionClosure, AdmissionFinding, AuthorityObservation,
            ClosureSummary, HeartbeatRetirement, PARENT_LOSS_CLOSURE_SCHEMA_V1,
            PARENT_LOSS_LEDGER_SCHEMA_V1, ParentLossGenerationRecord,
            ParentLossTerminalDisposition, ParentLossUnresolvedReason,
        };
        use solstone_core_system::process::{ProcessBirth, ProcessInstance};

        let instance = |pid| ProcessInstance {
            pid,
            birth: ProcessBirth::linux(1, 1, 100),
        };
        let journal = TempDir::new().expect("journal");
        let generation = journal.path().join("health/parent-loss/generations/8");
        fs::create_dir_all(generation.join("admissions/live-a")).unwrap();
        fs::create_dir_all(generation.join("admissions/live-b")).unwrap();
        fs::create_dir_all(generation.join("discharged/old.wedged-1.json")).unwrap();
        let record = ParentLossGenerationRecord {
            schema: PARENT_LOSS_LEDGER_SCHEMA_V1,
            generation: 7,
            coordinator: Some(instance(2)),
            supervisor: instance(1),
            sealed_ledger_digest: Some("d".to_owned()),
            terminal: Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::AuthoritiesLost,
            }),
            supervisor_heartbeat: None,
            closure: Some(AbandonedGenerationClosure {
                schema: PARENT_LOSS_CLOSURE_SCHEMA_V1,
                closed_by: instance(3),
                successor_generation: 8,
                closed_at: 1_789_000_000,
                supervisor: AuthorityObservation::Exited,
                coordinator: AuthorityObservation::Exited,
                heartbeat: HeartbeatRetirement::Removed,
                set_aside: Vec::new(),
                notes: Vec::new(),
                admissions: vec![AdmissionClosure {
                    launch_id: "cortex-a".to_owned(),
                    service: None,
                    instance: Some(instance(4)),
                    uid: Some(501),
                    finding: AdmissionFinding::Retired { escalated: false },
                }],
                summary: Some(ClosureSummary {
                    exited: 107_066,
                    retired: 1,
                    never_acknowledged: 1,
                    ..ClosureSummary::default()
                }),
            }),
        };
        let seven = journal.path().join("health/parent-loss/generations/7");
        fs::create_dir_all(&seven).unwrap();
        fs::write(
            seven.join("record.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let report = scan(journal.path(), ReportForm::Structured);
        assert!(
            report.closed_generations[0]
                .contains("107068 admissions, 107066 already exited, 1 stopped"),
            "{:?}",
            report.closed_generations
        );
        assert_eq!(
            report.closed_generations.len(),
            2,
            "one summary line, one noteworthy row"
        );
        assert_eq!(report.outstanding, Some((8, 2)));
        assert!(
            report.set_aside.is_empty(),
            "discharged/ is the prune's, not a heal"
        );
        let result = run(&test_context(journal.path().to_path_buf()), test_check()).expect("run");
        assert!(result.detail.contains("still owes 2 launches"));
    }

    #[cfg(unix)]
    #[test]
    fn a_lost_descendant_census_is_named_as_a_refused_start() {
        let journal = TempDir::new().expect("journal");
        let generation = journal.path().join("health/parent-loss/generations/4");
        fs::create_dir_all(&generation).unwrap();
        fs::write(generation.join("census-started.json"), b"{\"schema\":1}").unwrap();
        let report = scan(journal.path(), ReportForm::Structured);
        assert_eq!(report.refusals.len(), 1);
        assert!(report.refusals[0].contains("generation 4"));
        #[cfg(not(target_os = "macos"))]
        assert!(report.refusals[0].contains("it clears after the computer restarts"));
        let result = run(&test_context(journal.path().to_path_buf()), test_check()).expect("run");
        assert_eq!(result.status, Status::Fail);
        assert!(result.detail.contains("starts refused"));
    }
}
