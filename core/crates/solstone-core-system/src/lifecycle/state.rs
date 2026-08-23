// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The sole lifecycle leaf permitted to create, replace, or remove `health/` files.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::LifecycleError;
use super::readiness::ReadinessMarker;

fn health(journal: &Path) -> PathBuf {
    journal.join("health")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn open_supervisor_lock(journal: &Path) -> Result<File, LifecycleError> {
    fs::create_dir_all(health(journal))?;
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(health(journal).join("supervisor.lock"))?)
}

pub fn write_readiness(
    journal: &Path,
    ready_at: f64,
    mut extra: serde_json::Map<String, serde_json::Value>,
) -> Result<(), LifecycleError> {
    let pid = read_pid(&health(journal).join("supervisor.pid"))?;
    let start_time = read_f64(&health(journal).join("supervisor.start_time"))?;
    // These three fields are supervisor identity, never caller-controlled extras.
    extra.remove("pid");
    extra.remove("ready_at");
    extra.remove("start_time");
    let marker = ReadinessMarker {
        pid,
        ready_at,
        start_time,
        extra,
    };
    atomic_write(
        &health(journal).join("supervisor.ready"),
        &serde_json::to_vec(&marker)?,
    )
}

pub fn clear_ready(journal: &Path) -> Result<(), LifecycleError> {
    match fs::remove_file(health(journal).join("supervisor.ready")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn clear_supervisor_identity(journal: &Path) -> Result<(), LifecycleError> {
    for name in ["supervisor.pid", "supervisor.start_time"] {
        match fs::remove_file(health(journal).join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn clear_self_heartbeat(journal: &Path, filename: &str) -> Result<(), LifecycleError> {
    match fs::remove_file(heartbeat_path(journal, filename)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn write_sync_heartbeat(
    journal: &Path,
    filename: &str,
    body: &[u8],
) -> Result<(), LifecycleError> {
    atomic_write(&heartbeat_path(journal, filename)?, body)
}

fn heartbeat_path(journal: &Path, filename: &str) -> Result<PathBuf, LifecycleError> {
    let candidate = Path::new(filename);
    if filename.is_empty()
        || filename.starts_with('.')
        || !filename.ends_with(".check")
        || filename.contains(['/', '\\'])
        || candidate.file_name() != Some(OsStr::new(filename))
    {
        return Err(LifecycleError::InvalidHeartbeatFilename);
    }
    Ok(health(journal).join("sync").join(filename))
}

pub fn compact_log_if_oversized(log_path: &Path, max_bytes: u64) -> Result<(), LifecycleError> {
    let size = match log_path.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if size <= max_bytes {
        return Ok(());
    }
    let compact = log_path.with_file_name(format!(
        "{}.compact",
        log_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("supervisor.log")
    ));
    let result = (|| -> Result<(), LifecycleError> {
        let mut source = File::open(log_path)?;
        source.seek(SeekFrom::End(-(max_bytes as i64)))?;
        let mut tail = Vec::with_capacity(max_bytes as usize);
        source.read_to_end(&mut tail)?;
        let kept = tail
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| &tail[index + 1..])
            .unwrap_or_default();
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&compact)?;
        target.write_all(kept)?;
        target.flush()?;
        fs::rename(&compact, log_path)?;
        Ok(())
    })();
    if let Err(error) = &result {
        eprintln!("supervisor log compaction failed: {error}");
        let _ = fs::remove_file(&compact);
    }
    result
}

pub fn append_supervisor_log(
    log_path: &Path,
    message: &[u8],
    max_bytes: u64,
    backup_count: usize,
) -> Result<(), LifecycleError> {
    if log_path
        .metadata()
        .is_ok_and(|metadata| metadata.len() >= max_bytes)
    {
        for index in (1..=backup_count).rev() {
            let source = if index == 1 {
                log_path.to_path_buf()
            } else {
                log_path.with_extension((index - 1).to_string())
            };
            let target = log_path.with_extension(index.to_string());
            if source.exists() {
                let _ = fs::rename(source, target);
            }
        }
    }
    let parent = log_path
        .parent()
        .ok_or(LifecycleError::Identity("log parent"))?;
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    file.write_all(message)?;
    file.flush()?;
    Ok(())
}

fn atomic_write(path: &Path, body: &[u8]) -> Result<(), LifecycleError> {
    let parent = path
        .parent()
        .ok_or(LifecycleError::Identity("health path parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_file_name(format!(
        ".{}_{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state"),
        std::process::id()
    ));
    let result = (|| -> Result<(), LifecycleError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(body)?;
        file.flush()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn read_pid(path: &Path) -> Result<u32, LifecycleError> {
    fs::read_to_string(path)?
        .trim()
        .parse()
        .map_err(|_| LifecycleError::Identity("pid"))
}

pub fn recorded_supervisor_pid(journal: &Path) -> Option<u32> {
    read_pid(&health(journal).join("supervisor.pid")).ok()
}

fn read_f64(path: &Path) -> Result<f64, LifecycleError> {
    let value: f64 = fs::read_to_string(path)?
        .trim()
        .parse()
        .map_err(|_| LifecycleError::Identity("start time"))?;
    value
        .is_finite()
        .then_some(value)
        .ok_or(LifecycleError::Identity("start time"))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn process_start_time_epoch_seconds(pid: u32) -> Result<f64, LifecycleError> {
    use crate::process::{InspectResult, ProcessInstanceSource, SystemProcessInstanceSource};

    match SystemProcessInstanceSource.inspect(pid) {
        InspectResult::Present { instance, .. } => Ok(instance.birth.epoch_seconds()),
        InspectResult::Absent | InspectResult::Unverifiable => {
            Err(LifecycleError::Identity("process start time"))
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn write_supervisor_identity(journal: &Path, pid: u32) -> Result<(), LifecycleError> {
    let start_time = process_start_time_epoch_seconds(pid)?;
    atomic_write(
        &health(journal).join("supervisor.pid"),
        pid.to_string().as_bytes(),
    )?;
    atomic_write(
        &health(journal).join("supervisor.start_time"),
        start_time.to_string().as_bytes(),
    )
}

// Process start-time identity is owned by process::instance.
// iOS still has no supported process-start-time source.

#[cfg(test)]
pub(crate) fn test_supervisor_journal(
    name: &str,
    pid: u32,
    start_time: f64,
    marker: Option<&ReadinessMarker>,
) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("solstone-{name}-{stamp}"));
    let health = health(&root);
    fs::create_dir_all(&health).expect("health");
    fs::write(health.join("supervisor.pid"), pid.to_string()).expect("pid");
    fs::write(health.join("supervisor.start_time"), start_time.to_string()).expect("start");
    if let Some(marker) = marker {
        fs::write(
            health.join("supervisor.ready"),
            serde_json::to_vec(marker).expect("marker"),
        )
        .expect("ready");
    }
    root
}

#[cfg(test)]
pub(crate) fn remove_test_supervisor_journal(root: PathBuf) {
    fs::remove_dir_all(root).expect("cleanup");
}
