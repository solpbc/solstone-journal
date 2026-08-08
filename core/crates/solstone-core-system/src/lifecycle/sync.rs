// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::LifecycleError;

pub const DEFAULT_INTERVAL_SECONDS: f64 = 15.0;
pub const FRESH_WINDOW_MULTIPLIER: f64 = 4.0;
const FRESH_WINDOW_SECONDS: f64 = FRESH_WINDOW_MULTIPLIER * DEFAULT_INTERVAL_SECONDS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Heartbeat {
    pub schema: u8,
    pub machine_id: String,
    pub hostname: String,
    pub pid: u32,
    pub wall_time: String,
    pub solstone_version: String,
    pub interval_seconds: u32,
    pub journal_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSnapshot {
    pub files: BTreeMap<String, (u64, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignWriter {
    pub path: PathBuf,
    pub hostname: String,
    pub machine_id: String,
    pub journal_path: String,
    pub pid: Option<u32>,
    pub wall_time: String,
    pub is_live: bool,
    pub malformed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncConflictEvent {
    pub hostname: String,
    pub journal_path: String,
    pub pid: Option<u32>,
    pub machine_id_prefix: String,
    pub wall_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCheckResult {
    pub snapshot: SyncSnapshot,
    pub foreign_writers: Vec<ForeignWriter>,
    pub live_foreign_writers: Vec<ForeignWriter>,
}

impl SyncCheckResult {
    pub fn is_boot_conflict(&self) -> bool {
        !self.live_foreign_writers.is_empty()
    }
    pub fn is_tick_conflict(&self, previous: Option<&SyncSnapshot>) -> bool {
        previous.is_some() && !self.live_foreign_writers.is_empty()
    }
}

pub fn format_conflict_message(result: &SyncCheckResult) -> String {
    let Some(writer) = result.live_foreign_writers.first() else {
        return String::new();
    };
    format!(
        "Refusing to start - another solstone service is active on this journal.\n\
         Host: {}\nJournal: {}\nPID: {}\nMachine: {}",
        display_hostname(&writer.hostname),
        writer.journal_path,
        writer
            .pid
            .map_or_else(|| "(unknown)".to_owned(), |pid| pid.to_string()),
        machine_id_prefix(&writer.machine_id),
    )
}

pub fn sync_conflict_event(result: &SyncCheckResult) -> Option<SyncConflictEvent> {
    result
        .live_foreign_writers
        .first()
        .map(|writer| SyncConflictEvent {
            hostname: writer.hostname.clone(),
            journal_path: writer.journal_path.clone(),
            pid: writer.pid,
            machine_id_prefix: machine_id_prefix(&writer.machine_id),
            wall_time: writer.wall_time.clone(),
        })
}

pub fn machine_id() -> String {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/etc/machine-id")
            .map(|value| value.trim().to_owned())
            .unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        let Ok(output) = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        else {
            return String::new();
        };
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .find_map(|line| line.split_once("IOPlatformUUID"))
            .and_then(|(_, value)| value.split('"').nth(1))
            .unwrap_or_default()
            .to_owned()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        String::new()
    }
}

fn display_hostname(hostname: &str) -> &str {
    if hostname.is_empty() {
        "(unknown)"
    } else {
        hostname
    }
}

fn machine_id_prefix(machine_id: &str) -> String {
    if machine_id.is_empty() {
        "(unknown)".to_owned()
    } else {
        machine_id.chars().take(8).collect()
    }
}

pub fn sanitize_hostname(hostname: &str) -> String {
    let output: String = hostname
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect();
    output.trim_matches('-').to_owned().if_empty("unknown-host")
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}
impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_owned()
        } else {
            self
        }
    }
}

pub fn check(
    journal: &Path,
    self_filename: &str,
    self_machine_id: &str,
    previous: Option<&SyncSnapshot>,
    now: f64,
) -> Result<SyncCheckResult, LifecycleError> {
    let directory = journal.join("health").join("sync");
    let mut snapshot = SyncSnapshot::default();
    let mut foreign_writers = Vec::new();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SyncCheckResult {
                snapshot,
                foreign_writers,
                live_foreign_writers: Vec::new(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("check") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_secs());
        // The snapshot only compares same-process observations; it is not a
        // persisted cryptographic contract, so retain bytes without a new hash dependency.
        let digest = format!("{data:?}");
        snapshot.files.insert(name.clone(), (mtime, digest.clone()));
        let parsed: Result<Heartbeat, _> = serde_json::from_slice(&data);
        let malformed = parsed.is_err();
        let heartbeat = parsed.ok();
        if name == self_filename
            || heartbeat.as_ref().is_some_and(|value| {
                !self_machine_id.is_empty() && value.machine_id == self_machine_id
            })
        {
            continue;
        }
        let fresh = now - mtime as f64 <= FRESH_WINDOW_SECONDS;
        let prior = previous.and_then(|value| value.files.get(&name));
        let changed = prior.is_some_and(|value| value != &(mtime, digest.clone()));
        let appeared = previous.is_some() && prior.is_none();
        let live = if malformed {
            fresh
        } else {
            fresh || changed || appeared
        };
        foreign_writers.push(ForeignWriter {
            path,
            hostname: heartbeat
                .as_ref()
                .map_or_else(|| "(unknown)".to_owned(), |value| value.hostname.clone()),
            machine_id: heartbeat
                .as_ref()
                .map_or_else(String::new, |value| value.machine_id.clone()),
            journal_path: heartbeat
                .as_ref()
                .map_or_else(String::new, |value| value.journal_path.clone()),
            pid: heartbeat.as_ref().map(|value| value.pid),
            wall_time: heartbeat
                .as_ref()
                .map_or_else(String::new, |value| value.wall_time.clone()),
            is_live: live,
            malformed,
        });
    }
    let live_foreign_writers = foreign_writers
        .iter()
        .filter(|writer| writer.is_live)
        .cloned()
        .collect();
    Ok(SyncCheckResult {
        snapshot,
        foreign_writers,
        live_foreign_writers,
    })
}
