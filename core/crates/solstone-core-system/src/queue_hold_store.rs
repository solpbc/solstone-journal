// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Persistent on-disk storage and audit logging for task-queue partition holds.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::atomic::{AtomicWriteOptions, atomic_replace};
use solstone_core_journal_io::durability::ArtifactId;

use crate::partition::Partition;
use crate::process::{InstanceVerdict, ProcessBirth, ProcessInstance, ProcessInstanceSource};
use crate::queue_hold::{ReasonCode, ReleaseBasis};

/// Platform tag used for cross-platform hold classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldPlatform {
    Linux,
    Macos,
    Windows,
}

impl HoldPlatform {
    pub const fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(target_os = "macos")]
        {
            Self::Macos
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Self::Linux
        }
    }
}

/// One bound child identity persisted beside a task root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedBoundIdentity {
    pub pid: u32,
    pub birth: ProcessBirth,
    pub uid: u32,
}

/// On-disk record representing an in-flight or held task partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InFlightRecord {
    pub phase: String,
    pub hold_id: String,
    pub partition: String,
    pub references: Vec<String>,
    pub command: Vec<String>,
    pub day: Option<String>,
    pub scheduler_name: Option<String>,
    pub uid: u32,
    pub created_unix: u64,
    pub root: Option<ProcessInstance>,
    pub group_id: Option<u32>,
    pub bound: Vec<PersistedBoundIdentity>,
    pub exit_code: Option<i32>,
    pub reasons: Vec<ReasonCode>,
    pub termination_error: Option<String>,
    pub snapshot_unavailable: bool,
    pub held: bool,
}

/// One audit log line appended to `health/task-queue/holds.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldAuditRecord {
    pub event: String,
    pub hold_id: String,
    pub partition: String,
    pub references: Vec<String>,
    pub reasons: Vec<ReasonCode>,
    pub termination_error: Option<String>,
    pub snapshot_unavailable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basis: Option<ReleaseBasis>,
}

/// Parsed metadata from a supervisor scope directory name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedScope {
    pub boot_id_hex: Option<String>,
    pub supervisor: ProcessInstance,
}

/// Encode bytes into lowercase hex.
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Decode lowercase hex into bytes.
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16).ok()?;
        bytes.push(byte);
    }
    Some(bytes)
}

/// Generate a 32-character lowercase hex hold identifier from 16 random bytes.
pub fn generate_hold_id() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    hex_encode(&bytes)
}

/// Read the current host boot identity string.
pub fn current_boot_identity() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|text| text.trim().to_owned())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "kern.bootsessionuuid"])
            .output()
            .ok()
            .and_then(|output| {
                if output.status.success() {
                    String::from_utf8(output.stdout)
                        .ok()
                        .map(|text| text.trim().to_owned())
                } else {
                    None
                }
            })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Encode process birth for scope directory path formatting.
///
/// Linux formats as `bl-{start_ticks}-{btime}-{clk_tck}`.
/// macOS formats as `bm-{epoch_micros}` or `bm-n{abs}` if negative.
/// Windows formats as `bw-{filetime}`.
pub fn format_process_birth(birth: &ProcessBirth) -> String {
    #[cfg(target_os = "linux")]
    {
        if let Some((ticks, btime, clk)) = birth.linux_tuple() {
            return format!("bl-{ticks}-{btime}-{clk}");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(micros) = birth.macos_micros() {
            return if micros < 0 {
                format!("bm-n{}", micros.abs())
            } else {
                format!("bm-{micros}")
            };
        }
    }
    #[cfg(windows)]
    {
        if let Some(ft) = birth.windows_filetime() {
            return format!("bw-{ft}");
        }
    }
    // Fallback when inspecting synthetic or other-platform births:
    if let Some((ticks, btime, clk)) = birth.linux_tuple() {
        format!("bl-{ticks}-{btime}-{clk}")
    } else if let Some(micros) = birth.macos_micros() {
        if micros < 0 {
            format!("bm-n{}", micros.abs())
        } else {
            format!("bm-{micros}")
        }
    } else if let Some(ft) = birth.windows_filetime() {
        format!("bw-{ft}")
    } else {
        "b-unknown".to_owned()
    }
}

/// Parse process birth token from string format.
pub fn parse_process_birth(s: &str) -> Option<ProcessBirth> {
    if let Some(rest) = s.strip_prefix("bl-") {
        let mut parts = rest.split('-');
        let ticks = parts.next()?.parse::<u64>().ok()?;
        let btime = parts.next()?.parse::<u64>().ok()?;
        let clk = parts.next()?.parse::<u64>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        #[cfg(target_os = "linux")]
        {
            Some(ProcessBirth::linux(ticks, btime, clk))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Some(ProcessBirth::linux(ticks, btime, clk))
        }
    } else if let Some(rest) = s.strip_prefix("bm-") {
        let micros = if let Some(neg_rest) = rest.strip_prefix('n') {
            let val = neg_rest.parse::<i64>().ok()?;
            -val
        } else {
            rest.parse::<i64>().ok()?
        };
        #[cfg(target_os = "macos")]
        {
            Some(ProcessBirth::macos(micros))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Some(ProcessBirth::macos(micros))
        }
    } else if let Some(rest) = s.strip_prefix("bw-") {
        let ft = rest.parse::<u64>().ok()?;
        #[cfg(windows)]
        {
            Some(ProcessBirth::windows(ft))
        }
        #[cfg(not(windows))]
        {
            Some(ProcessBirth::windows(ft))
        }
    } else if s == "b-unknown" {
        Some(ProcessBirth::unknown())
    } else {
        None
    }
}

/// Build a scope directory name from boot identity and supervisor process instance.
///
/// On Linux/macOS: `b-{boot_hex}--p-{pid}--{birth}`.
/// On Windows: `p-{pid}--{birth}`.
pub fn format_scope_dir_name(boot_id: Option<&str>, supervisor: &ProcessInstance) -> String {
    let birth_str = format_process_birth(&supervisor.birth);
    if let Some(boot) = boot_id {
        let boot_hex = hex_encode(boot.trim().as_bytes());
        format!("b-{boot_hex}--p-{}--{birth_str}", supervisor.pid)
    } else {
        format!("p-{}--{birth_str}", supervisor.pid)
    }
}

/// Parse a scope directory name into boot ID hex and supervisor process instance.
pub fn parse_scope_dir_name(name: &str) -> Option<ParsedScope> {
    let parts: Vec<&str> = name.split("--").collect();
    if parts.len() == 3 {
        // b-{boot_hex}, p-{pid}, {birth}
        let boot_hex = parts[0].strip_prefix("b-")?.to_owned();
        let pid = parts[1].strip_prefix("p-")?.parse::<u32>().ok()?;
        let birth = parse_process_birth(parts[2])?;
        Some(ParsedScope {
            boot_id_hex: Some(boot_hex),
            supervisor: ProcessInstance { pid, birth },
        })
    } else if parts.len() == 2 {
        // p-{pid}, {birth}
        let pid = parts[0].strip_prefix("p-")?.parse::<u32>().ok()?;
        let birth = parse_process_birth(parts[1])?;
        Some(ParsedScope {
            boot_id_hex: None,
            supervisor: ProcessInstance { pid, birth },
        })
    } else {
        None
    }
}

/// Path to `health/task-queue/in-flight` directory.
pub fn in_flight_directory(journal_root: &Path) -> PathBuf {
    journal_root
        .join("health")
        .join("task-queue")
        .join("in-flight")
}

/// Path to `health/task-queue/in-flight/<scope>` directory.
pub fn scope_directory(journal_root: &Path, scope: &str) -> PathBuf {
    in_flight_directory(journal_root).join(scope)
}

/// Hex-encoded file name for a partition: `<hex>.json`.
pub fn partition_file_name(partition: &Partition) -> String {
    format!("{}.json", hex_encode(partition.as_str().as_bytes()))
}

/// Path to `health/task-queue/in-flight/<scope>/<partition_hex>.json`.
pub fn partition_record_path(journal_root: &Path, scope: &str, partition: &Partition) -> PathBuf {
    scope_directory(journal_root, scope).join(partition_file_name(partition))
}

/// Path to audit log `health/task-queue/holds.jsonl`.
pub fn audit_log_path(journal_root: &Path) -> PathBuf {
    journal_root
        .join("health")
        .join("task-queue")
        .join("holds.jsonl")
}

/// Read an in-flight record from a file.
pub fn read_in_flight_record(path: &Path) -> io::Result<InFlightRecord> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Atomically write an in-flight record.
pub fn write_in_flight_record(path: &Path, record: &InFlightRecord) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    atomic_replace(path, &bytes, AtomicWriteOptions::default())
        .map_err(|e| io::Error::other(e.to_string()))
}

/// Append an audit line to `health/task-queue/holds.jsonl`.
pub fn append_hold_audit(journal_root: &Path, record: &HoldAuditRecord) -> io::Result<()> {
    // Artifact declaration mention required by repository durability contract:
    let _ = ArtifactId::TaskQueueHolds;
    let path = audit_log_path(journal_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    solstone_core_journal_io::append_jsonl(&path, record)
        .map_err(|e| io::Error::other(e.to_string()))
}

/// Finding reported by doctor task_queue_holds check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskQueueHoldFinding {
    pub path: PathBuf,
    pub is_fail: bool,
    pub is_warn: bool,
    pub detail: String,
    pub action: Option<String>,
}

/// Classify on-disk task-queue holds across all scopes (read-only).
pub fn classify_task_queue_holds(
    journal_root: &Path,
    processes: &dyn ProcessInstanceSource,
    current_boot: Option<&str>,
    platform: HoldPlatform,
) -> Vec<TaskQueueHoldFinding> {
    let in_flight = in_flight_directory(journal_root);
    if !in_flight.exists() {
        return Vec::new();
    }

    let mut findings = Vec::new();
    let current_boot_hex = current_boot.map(|b| hex_encode(b.trim().as_bytes()));

    let entries = match fs::read_dir(&in_flight) {
        Ok(e) => e,
        Err(err) => {
            findings.push(TaskQueueHoldFinding {
                path: in_flight,
                is_fail: true,
                is_warn: false,
                detail: format!("cannot list in-flight holds directory: {err}"),
                action: None,
            });
            return findings;
        }
    };

    for entry in entries.flatten() {
        let scope_path = entry.path();
        let file_name = entry.file_name();
        let scope_name = file_name.to_string_lossy();

        if !scope_path.is_dir() {
            let action = format!("remove {}: it holds nothing", scope_path.display());
            findings.push(TaskQueueHoldFinding {
                path: scope_path,
                is_fail: false,
                is_warn: true,
                detail: "stray non-directory in task-queue in-flight path".to_owned(),
                action: Some(action),
            });
            continue;
        }

        let parsed_scope = match parse_scope_dir_name(&scope_name) {
            Some(s) => s,
            None => {
                findings.push(TaskQueueHoldFinding {
                    path: scope_path,
                    is_fail: false,
                    is_warn: true,
                    detail: "unrecognized scope directory format; the journal still holds any task recorded in it"
                        .to_owned(),
                    action: None,
                });
                continue;
            }
        };

        // If boot id is known and different, this entire scope is from an earlier boot and not reported:
        if let (Some(scope_b), Some(curr_b)) = (&parsed_scope.boot_id_hex, &current_boot_hex)
            && scope_b != curr_b
        {
            continue;
        }

        let scope_entries = match fs::read_dir(&scope_path) {
            Ok(e) => e,
            Err(err) => {
                findings.push(TaskQueueHoldFinding {
                    path: scope_path,
                    is_fail: true,
                    is_warn: false,
                    detail: format!("cannot list scope directory: {err}"),
                    action: None,
                });
                continue;
            }
        };

        for rec_entry in scope_entries.flatten() {
            let rec_path = rec_entry.path();
            let rec_name = rec_entry.file_name();
            let rec_name_str = rec_name.to_string_lossy();

            // An atomic write in progress; the loader skips these too.
            if rec_name_str.ends_with(".tmp") {
                continue;
            }
            let is_json_file = rec_name_str.ends_with(".json") && !rec_path.is_dir();
            if !is_json_file {
                let action = format!("remove {}: it holds nothing", rec_path.display());
                findings.push(TaskQueueHoldFinding {
                    path: rec_path,
                    is_fail: false,
                    is_warn: true,
                    detail: "non-record entry in scope directory".to_owned(),
                    action: Some(action),
                });
                continue;
            }

            // Check if file name hex decodes
            let hex_stem = rec_name_str.trim_end_matches(".json");
            if hex_decode(hex_stem).is_none() {
                findings.push(TaskQueueHoldFinding {
                    path: rec_path,
                    is_fail: false,
                    is_warn: true,
                    detail: "invalid partition file name in scope directory; the journal still reads it as a task record"
                        .to_owned(),
                    action: None,
                });
                continue;
            }

            let bytes = match fs::read(&rec_path) {
                Ok(b) => b,
                Err(err) => {
                    findings.push(TaskQueueHoldFinding {
                        path: rec_path,
                        is_fail: true,
                        is_warn: false,
                        detail: format!("cannot read record: {err}"),
                        action: None,
                    });
                    continue;
                }
            };

            if bytes.is_empty() {
                findings.push(TaskQueueHoldFinding {
                    path: rec_path,
                    is_fail: true,
                    is_warn: false,
                    detail: "record file is empty".to_owned(),
                    action: None,
                });
                continue;
            }

            let record: InFlightRecord = match serde_json::from_slice(&bytes) {
                Ok(r) => r,
                Err(err) => {
                    findings.push(TaskQueueHoldFinding {
                        path: rec_path,
                        is_fail: true,
                        is_warn: false,
                        detail: format!("cannot parse record: {err}"),
                        action: None,
                    });
                    continue;
                }
            };

            if record.held {
                if matches!(platform, HoldPlatform::Windows)
                    && matches!(
                        processes.observe(&parsed_scope.supervisor),
                        InstanceVerdict::NotSameOrExited
                    )
                {
                    // The supervisor that held it is gone, so its kill-on-close
                    // Jobs closed and the next start releases this hold.
                    continue;
                }
                findings.push(TaskQueueHoldFinding {
                    path: rec_path,
                    is_fail: false,
                    is_warn: true,
                    detail: format!("held partition {}", record.partition),
                    action: None,
                });
            } else {
                let verdict = processes.observe(&parsed_scope.supervisor);
                if matches!(verdict, InstanceVerdict::SameLive { .. }) {
                    // Running supervisor is actively executing this unheld task.
                    continue;
                }
                match platform {
                    HoldPlatform::Linux | HoldPlatform::Macos => {
                        findings.push(TaskQueueHoldFinding {
                            path: rec_path,
                            is_fail: false,
                            is_warn: true,
                            detail: format!(
                                "partition {} was running when the journal stopped; the journal holds it at its next start until its processes are gone",
                                record.partition
                            ),
                            action: None,
                        });
                    }
                    HoldPlatform::Windows => {
                        // On Windows, the supervisor's death closed its kill-on-close Jobs.
                    }
                }
            }
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ExecutionState;

    fn test_birth(token: u64) -> ProcessBirth {
        #[cfg(target_os = "linux")]
        {
            ProcessBirth::linux(token, 0, 100)
        }
        #[cfg(target_os = "macos")]
        {
            ProcessBirth::macos(token as i64)
        }
        #[cfg(windows)]
        {
            ProcessBirth::windows(token)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            ProcessBirth::linux(token, 0, 100)
        }
    }

    struct MockProcessSource {
        verdict: InstanceVerdict,
    }

    impl ProcessInstanceSource for MockProcessSource {
        fn inspect(&self, _pid: u32) -> crate::process::InspectResult {
            crate::process::InspectResult::Unverifiable
        }
        fn census(&self) -> crate::process::InstanceCensus {
            crate::process::InstanceCensus::Incomplete(Vec::new())
        }
        fn observe(&self, _expected: &ProcessInstance) -> InstanceVerdict {
            self.verdict
        }
    }

    #[test]
    fn scope_dir_formatting_and_parsing_round_trip() {
        let supervisor = ProcessInstance {
            pid: 42,
            birth: test_birth(12345),
        };
        let formatted = format_scope_dir_name(Some("my-boot-id"), &supervisor);
        let parsed = parse_scope_dir_name(&formatted).expect("parsed scope");
        assert_eq!(parsed.supervisor, supervisor);
        assert_eq!(parsed.boot_id_hex, Some(hex_encode(b"my-boot-id")));

        let formatted_no_boot = format_scope_dir_name(None, &supervisor);
        let parsed_no_boot = parse_scope_dir_name(&formatted_no_boot).expect("parsed no boot");
        assert_eq!(parsed_no_boot.supervisor, supervisor);
        assert_eq!(parsed_no_boot.boot_id_hex, None);
    }

    #[test]
    fn classification_different_boot_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path();
        let scope_name = format!("b-{}--p-10--bl-1-0-100", hex_encode(b"old-boot"));
        let scope = scope_directory(journal, &scope_name);
        fs::create_dir_all(&scope).unwrap();
        let rec_path = scope.join(format!("{}.json", hex_encode(b"svc")));
        let record = InFlightRecord {
            phase: "running".into(),
            hold_id: generate_hold_id(),
            partition: "svc".into(),
            references: vec!["ref1".into()],
            command: vec!["think".into()],
            day: None,
            scheduler_name: None,
            uid: 1000,
            created_unix: 1234,
            root: None,
            group_id: None,
            bound: vec![],
            exit_code: None,
            reasons: vec![ReasonCode::RootLive],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        fs::write(&rec_path, serde_json::to_vec(&record).unwrap()).unwrap();

        let source = MockProcessSource {
            verdict: InstanceVerdict::NotSameOrExited,
        };
        let findings =
            classify_task_queue_holds(journal, &source, Some("new-boot"), HoldPlatform::Linux);
        assert!(findings.is_empty());
    }

    #[test]
    fn classification_non_record_file_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path();
        let in_flight = in_flight_directory(journal);
        fs::create_dir_all(&in_flight).unwrap();
        fs::write(in_flight.join("stray.txt"), b"debris").unwrap();

        let source = MockProcessSource {
            verdict: InstanceVerdict::NotSameOrExited,
        };
        let findings =
            classify_task_queue_holds(journal, &source, Some("boot"), HoldPlatform::Linux);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_warn);
        assert!(findings[0].action.is_some());
    }

    #[test]
    fn classification_held_record_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path();
        let scope_name = format!("b-{}--p-10--bl-1-0-100", hex_encode(b"boot"));
        let scope = scope_directory(journal, &scope_name);
        fs::create_dir_all(&scope).unwrap();
        let rec_path = scope.join(format!("{}.json", hex_encode(b"svc")));
        let record = InFlightRecord {
            phase: "running".into(),
            hold_id: generate_hold_id(),
            partition: "svc".into(),
            references: vec!["ref1".into()],
            command: vec!["think".into()],
            day: None,
            scheduler_name: None,
            uid: 1000,
            created_unix: 1234,
            root: None,
            group_id: None,
            bound: vec![],
            exit_code: None,
            reasons: vec![ReasonCode::RootLive],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        fs::write(&rec_path, serde_json::to_vec(&record).unwrap()).unwrap();

        let source = MockProcessSource {
            verdict: InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        };
        let findings =
            classify_task_queue_holds(journal, &source, Some("boot"), HoldPlatform::Linux);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_warn);
    }

    fn held_record(partition: &str) -> InFlightRecord {
        InFlightRecord {
            phase: "running".into(),
            hold_id: generate_hold_id(),
            partition: partition.into(),
            references: vec!["ref1".into()],
            command: vec!["think".into()],
            day: None,
            scheduler_name: None,
            uid: 1000,
            created_unix: 1234,
            root: None,
            group_id: None,
            bound: vec![],
            exit_code: None,
            reasons: vec![ReasonCode::RootLive],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        }
    }

    #[test]
    fn classification_windows_held_record_of_a_gone_supervisor_is_not_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path();
        let scope = scope_directory(
            journal,
            &format!("b-{}--p-10--bl-1-0-100", hex_encode(b"boot")),
        );
        fs::create_dir_all(&scope).unwrap();
        fs::write(
            scope.join(format!("{}.json", hex_encode(b"svc"))),
            serde_json::to_vec(&held_record("svc")).unwrap(),
        )
        .unwrap();

        let gone = MockProcessSource {
            verdict: InstanceVerdict::NotSameOrExited,
        };
        assert!(
            classify_task_queue_holds(journal, &gone, Some("boot"), HoldPlatform::Windows)
                .is_empty()
        );
        let live = MockProcessSource {
            verdict: InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        };
        let findings =
            classify_task_queue_holds(journal, &live, Some("boot"), HoldPlatform::Windows);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_warn);
    }

    #[test]
    fn classification_skips_a_write_in_progress_and_never_offers_to_remove_a_read_record() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path();
        let scope = scope_directory(
            journal,
            &format!("b-{}--p-10--bl-1-0-100", hex_encode(b"boot")),
        );
        fs::create_dir_all(&scope).unwrap();
        fs::write(scope.join(".candidate.json.tmp"), b"partial").unwrap();
        fs::write(
            scope.join("not-hex.json"),
            serde_json::to_vec(&held_record("svc")).unwrap(),
        )
        .unwrap();
        let odd_scope = in_flight_directory(journal).join("odd-scope");
        fs::create_dir_all(&odd_scope).unwrap();

        let source = MockProcessSource {
            verdict: InstanceVerdict::NotSameOrExited,
        };
        let findings =
            classify_task_queue_holds(journal, &source, Some("boot"), HoldPlatform::Linux);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().all(|finding| finding.action.is_none()));
        assert!(
            findings
                .iter()
                .all(|finding| !finding.path.to_string_lossy().ends_with(".tmp"))
        );
    }

    #[test]
    fn classification_unheld_supervisor_gone_unix_warns_windows_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path();
        let scope_name = format!("b-{}--p-10--bl-1-0-100", hex_encode(b"boot"));
        let scope = scope_directory(journal, &scope_name);
        fs::create_dir_all(&scope).unwrap();
        let rec_path = scope.join(format!("{}.json", hex_encode(b"svc")));
        let record = InFlightRecord {
            phase: "running".into(),
            hold_id: generate_hold_id(),
            partition: "svc".into(),
            references: vec!["ref1".into()],
            command: vec!["think".into()],
            day: None,
            scheduler_name: None,
            uid: 1000,
            created_unix: 1234,
            root: None,
            group_id: None,
            bound: vec![],
            exit_code: None,
            reasons: vec![],
            termination_error: None,
            snapshot_unavailable: false,
            held: false,
        };
        fs::write(&rec_path, serde_json::to_vec(&record).unwrap()).unwrap();

        let source = MockProcessSource {
            verdict: InstanceVerdict::NotSameOrExited,
        };
        let unix_findings =
            classify_task_queue_holds(journal, &source, Some("boot"), HoldPlatform::Linux);
        assert_eq!(unix_findings.len(), 1);
        assert!(unix_findings[0].is_warn);

        let win_findings =
            classify_task_queue_holds(journal, &source, Some("boot"), HoldPlatform::Windows);
        assert!(win_findings.is_empty());
    }

    #[test]
    fn classification_unheld_supervisor_live_is_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path();
        let scope_name = format!("b-{}--p-10--bl-1-0-100", hex_encode(b"boot"));
        let scope = scope_directory(journal, &scope_name);
        fs::create_dir_all(&scope).unwrap();
        let rec_path = scope.join(format!("{}.json", hex_encode(b"svc")));
        let record = InFlightRecord {
            phase: "running".into(),
            hold_id: generate_hold_id(),
            partition: "svc".into(),
            references: vec!["ref1".into()],
            command: vec!["think".into()],
            day: None,
            scheduler_name: None,
            uid: 1000,
            created_unix: 1234,
            root: None,
            group_id: None,
            bound: vec![],
            exit_code: None,
            reasons: vec![],
            termination_error: None,
            snapshot_unavailable: false,
            held: false,
        };
        fs::write(&rec_path, serde_json::to_vec(&record).unwrap()).unwrap();

        let source = MockProcessSource {
            verdict: InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        };
        let findings =
            classify_task_queue_holds(journal, &source, Some("boot"), HoldPlatform::Linux);
        assert!(findings.is_empty());
    }

    #[test]
    fn classification_empty_record_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path();
        let scope_name = format!("b-{}--p-10--bl-1-0-100", hex_encode(b"boot"));
        let scope = scope_directory(journal, &scope_name);
        fs::create_dir_all(&scope).unwrap();
        let rec_path = scope.join(format!("{}.json", hex_encode(b"svc")));
        fs::write(&rec_path, b"").unwrap();

        let source = MockProcessSource {
            verdict: InstanceVerdict::NotSameOrExited,
        };
        let findings =
            classify_task_queue_holds(journal, &source, Some("boot"), HoldPlatform::Linux);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_fail);
        assert!(
            findings[0].detail.contains(&rec_path.display().to_string())
                || findings[0].path == rec_path
        );
    }

    #[test]
    fn classification_invalid_json_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path();
        let scope_name = format!("b-{}--p-10--bl-1-0-100", hex_encode(b"boot"));
        let scope = scope_directory(journal, &scope_name);
        fs::create_dir_all(&scope).unwrap();
        let rec_path = scope.join(format!("{}.json", hex_encode(b"svc")));
        fs::write(&rec_path, b"{invalid json").unwrap();

        let source = MockProcessSource {
            verdict: InstanceVerdict::NotSameOrExited,
        };
        let findings =
            classify_task_queue_holds(journal, &source, Some("boot"), HoldPlatform::Linux);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_fail);
    }

    #[test]
    fn classification_listing_failure_names_the_path() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if nix::unistd::geteuid().as_raw() == 0 {
                eprintln!("test running as root, cannot test directory permission denial");
                return;
            }
            let tmp = tempfile::tempdir().unwrap();
            let journal = tmp.path();
            let scope_name = format!("b-{}--p-10--bl-1-0-100", hex_encode(b"boot"));
            let scope = scope_directory(journal, &scope_name);
            fs::create_dir_all(&scope).unwrap();
            fs::set_permissions(&scope, fs::Permissions::from_mode(0o000)).unwrap();

            let source = MockProcessSource {
                verdict: InstanceVerdict::NotSameOrExited,
            };
            let findings =
                classify_task_queue_holds(journal, &source, Some("boot"), HoldPlatform::Linux);

            let _ = fs::set_permissions(&scope, fs::Permissions::from_mode(0o755));

            assert_eq!(findings.len(), 1);
            assert!(findings[0].is_fail);
            assert!(
                findings[0].path == scope
                    || findings[0].detail.contains(&scope.display().to_string())
            );
        }
    }
}
