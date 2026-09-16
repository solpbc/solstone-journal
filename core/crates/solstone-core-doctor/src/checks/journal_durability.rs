// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Names every piece of bookkeeping the journal healed rather than refused:
//! artifacts set aside as `<name>.wedged-<stamp>`, lifecycle generations a
//! successor closed because both their authorities died, and the set-aside
//! `health/parent-loss.wedged-*` trees an operator or an older heal left.
//! Nothing here is a fault to fix; it is the visible half of a heal.

use std::fs;
use std::path::Path;

use crate::context::CheckContext;
use crate::vocabulary::{Check, RunnerResult, Status, make_result, truncate};

const SET_ASIDE_MARKER: &str = ".wedged-";
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
    /// What could not be inspected.
    pub unreadable: Vec<String>,
}

impl DurabilityReport {
    fn is_clean(&self) -> bool {
        self.set_aside.is_empty()
            && self.closed_generations.is_empty()
            && self.unresolved_generations.is_empty()
            && self.unreadable.is_empty()
    }
}

fn relative(journal: &Path, path: &Path) -> String {
    path.strip_prefix(journal)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Collect set-aside names in one directory, non-recursively.
fn collect_set_aside(journal: &Path, directory: &Path, report: &mut DurabilityReport) {
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
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().contains(SET_ASIDE_MARKER) {
            report.set_aside.push(relative(journal, &entry.path()));
        }
    }
}

#[cfg(unix)]
fn scan_generations(journal: &Path, report: &mut DurabilityReport) {
    use solstone_core_system::lifecycle::{
        AdmissionFinding, ParentLossGenerationRecord, ParentLossTerminalDisposition,
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
    let mut numbered = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u64>().ok())
                .map(|generation| (generation, entry.path()))
        })
        .collect::<Vec<_>>();
    numbered.sort();
    for (generation, directory) in numbered {
        collect_set_aside(journal, &directory, report);
        let record_path = directory.join("record.json");
        let bytes = match fs::read(&record_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                report
                    .unreadable
                    .push(format!("{}: {error}", relative(journal, &record_path)));
                continue;
            }
        };
        let record: ParentLossGenerationRecord = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(error) => {
                report
                    .unreadable
                    .push(format!("{}: {error}", relative(journal, &record_path)));
                continue;
            }
        };
        if let Some(closure) = record.closure.as_ref() {
            let exited = closure
                .admissions
                .iter()
                .filter(|admission| admission.finding == AdmissionFinding::Exited)
                .count();
            let retired = closure
                .admissions
                .iter()
                .filter(|admission| matches!(admission.finding, AdmissionFinding::Retired { .. }))
                .count();
            let never_admitted = closure.admissions.len() - exited - retired;
            report.closed_generations.push(format!(
                "generation {generation} was closed by the start that became generation {} \
                 (unix {}): {} admissions, {exited} already exited, {retired} stopped by the journal, \
                 {never_admitted} never admitted",
                closure.successor_generation,
                closure.closed_at,
                closure.admissions.len(),
            ));
        } else if let Some(ParentLossTerminalDisposition::Unresolved { reason }) =
            record.terminal.as_ref()
        {
            report
                .unresolved_generations
                .push(format!("generation {generation}: {reason}"));
        }
    }
}

#[cfg(not(unix))]
fn scan_generations(_journal: &Path, _report: &mut DurabilityReport) {}

pub(crate) fn scan(journal: &Path) -> DurabilityReport {
    let mut report = DurabilityReport::default();
    let health = journal.join("health");
    collect_set_aside(journal, &journal.join("config"), &mut report);
    collect_set_aside(journal, &health, &mut report);
    collect_set_aside(journal, &health.join("parent-loss"), &mut report);
    scan_generations(journal, &mut report);
    report.set_aside.sort();
    report
}

fn named(kind: &str, items: &[String]) -> String {
    let mut lines = vec![format!("{kind} ({}):", items.len())];
    for item in items.iter().take(NAMED_LIMIT) {
        lines.push(format!("  {item}"));
    }
    if items.len() > NAMED_LIMIT {
        lines.push(format!("  ... and {} more", items.len() - NAMED_LIMIT));
    }
    lines.join("\n")
}

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            "no local journal",
            None::<String>,
        ));
    }
    let report = scan(&context.journal_path);
    if report.is_clean() {
        return Ok(make_result(
            check,
            Status::Ok,
            "no bookkeeping has needed healing",
            None::<String>,
        ));
    }
    let mut sections = Vec::new();
    if !report.closed_generations.is_empty() {
        sections.push(named(
            "runs closed by a later start",
            &report.closed_generations,
        ));
    }
    if !report.unresolved_generations.is_empty() {
        sections.push(named(
            "runs that ended without a proven clean stop",
            &report.unresolved_generations,
        ));
    }
    if !report.set_aside.is_empty() {
        sections.push(named("bookkeeping set aside", &report.set_aside));
    }
    if !report.unreadable.is_empty() {
        sections.push(named("could not inspect", &report.unreadable));
    }
    let detail = sections.join("\n");
    let status = if report.unreadable.is_empty() {
        Status::Warn
    } else {
        Status::Fail
    };
    Ok(make_result(
        check,
        status,
        truncate(&detail, 4096),
        Some(
            "nothing needs doing: these records are kept beside your journal's bookkeeping so a \
             heal is never silent; they hold none of your memories",
        ),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_JOURNAL: AtomicUsize = AtomicUsize::new(0);

    /// The doctor crate carries no temp-dir dependency; mirror `lib.rs`.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> std::io::Result<Self> {
            let root = std::env::temp_dir().join(format!(
                "solstone-doctor-durability-{}-{}",
                std::process::id(),
                NEXT_JOURNAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root)?;
            Ok(Self(root))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_journal_without_heals_is_clean() {
        let journal = TempDir::new().expect("journal");
        fs::create_dir_all(journal.path().join("health/parent-loss/generations/1")).unwrap();
        assert!(scan(journal.path()).is_clean());
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
        let report = scan(journal.path());
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

        let report = scan(journal.path());
        assert_eq!(
            report.closed_generations,
            vec![
                "generation 93 was closed by the start that became generation 94 (unix \
                 1789000000): 3 admissions, 1 already exited, 1 stopped by the journal, 1 never \
                 admitted"
                    .to_owned()
            ]
        );
        assert_eq!(report.unresolved_generations.len(), 1);
        assert!(report.unresolved_generations[0].starts_with("generation 6: "));
        assert!(!report.is_clean());
    }

    #[test]
    fn an_unreadable_record_is_reported_not_fatal() {
        let journal = TempDir::new().expect("journal");
        let generation = journal.path().join("health/parent-loss/generations/2");
        fs::create_dir_all(&generation).unwrap();
        fs::write(generation.join("record.json"), b"{ not json").unwrap();
        let report = scan(journal.path());
        #[cfg(unix)]
        assert_eq!(report.unreadable.len(), 1);
        #[cfg(not(unix))]
        assert!(report.unreadable.is_empty());
    }
}
