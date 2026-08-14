// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use chrono::{Local, NaiveDateTime, Utc};
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use nix::sys::signal::kill;
use nix::unistd::Pid;
use serde_json::json;
use solstone_core_steward_prune::{Disposition, classify_prune};

const RECENCY_WINDOW_HOURS: i64 = 12;

pub(crate) fn run(journal: &Path, force: bool) -> ExitCode {
    match run_inner(journal, force) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("journal heartbeat: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_inner(journal: &Path, force: bool) -> io::Result<()> {
    let health_dir = journal.join("health");
    fs::create_dir_all(&health_dir)?;

    if !force && recently_succeeded(&health_dir, Local::now().naive_local())? {
        return Ok(());
    }

    let pid_path = health_dir.join("heartbeat.pid");
    let result = guarded_pass(&health_dir, &pid_path);
    // The reference unlinks from its outer finally block on every guarded path.
    let cleanup = remove_if_present(&pid_path);
    result.and(cleanup)
}

fn recently_succeeded(health_dir: &Path, now: NaiveDateTime) -> io::Result<bool> {
    let contents = match fs::read_to_string(health_dir.join("heartbeat.log")) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Err(error),
        Err(_) => return Ok(false),
    };
    Ok(contents.lines().rev().find_map(|line| {
        if !line.contains("outcome=success") {
            return None;
        }
        let stamp = line.split_whitespace().next()?;
        let parsed = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
            .iter()
            .find_map(|format| NaiveDateTime::parse_from_str(stamp, format).ok())?;
        let seconds = now.signed_duration_since(parsed).num_seconds();
        Some(seconds < RECENCY_WINDOW_HOURS * 3600)
    }) == Some(true))
}

fn guarded_pass(health_dir: &Path, pid_path: &Path) -> io::Result<()> {
    match fs::read_to_string(pid_path) {
        Ok(raw) => match raw.trim().parse::<i32>() {
            Ok(pid) => match kill(Pid::from_raw(pid), None) {
                Ok(()) | Err(Errno::EPERM) => return Ok(()),
                Err(Errno::ESRCH) => remove_if_present(pid_path)?,
                Err(error) => return Err(io::Error::other(error)),
            },
            Err(_) => remove_if_present(pid_path)?,
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    fs::write(pid_path, std::process::id().to_string())?;
    let started = Instant::now();
    let pass = run_pass(health_dir);
    let outcome = if pass.is_ok() { "success" } else { "error" };
    let log_result = append_heartbeat_log(health_dir, started.elapsed().as_secs(), outcome);
    pass.and(log_result)
}

fn run_pass(health_dir: &Path) -> io::Result<()> {
    let steward_path = health_dir.join("steward.log");
    let record = json!({
        "data_source_errors": [],
        "escalated_targets": [],
        "event": "pass",
        "fired": [],
        "ts": Utc::now().timestamp_millis(),
    });
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&steward_path)?;
    serde_json::to_writer(&mut file, &record).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    prune_steward_log(health_dir, &steward_path);
    Ok(())
}

fn prune_steward_log(health_dir: &Path, steward_path: &Path) {
    let result = (|| -> io::Result<()> {
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(health_dir.join(".steward.lock"))?;
        let _lock = match Flock::lock(lock, FlockArg::LockExclusiveNonblock) {
            Ok(lock) => lock,
            Err((_file, Errno::EAGAIN)) => return Ok(()),
            Err((_file, error)) => return Err(io::Error::other(error)),
        };
        let input = match fs::read(steward_path) {
            Ok(input) => input,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let classified = classify_prune(&input, Utc::now().timestamp_millis());
        if !matches!(classified.disposition, Disposition::Rewrite { .. }) {
            return Ok(());
        }
        atomic_replace(health_dir, steward_path, &classified.output)
    })();
    if let Err(error) = result {
        eprintln!("journal heartbeat: steward log prune failed: {error}");
    }
}

fn atomic_replace(directory: &Path, target: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = unique_temporary_path(directory);
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.flush()?;
        fs::rename(&temporary, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn unique_temporary_path(directory: &Path) -> PathBuf {
    directory.join(format!(
        ".steward_{}_{}.tmp",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ))
}

fn append_heartbeat_log(health_dir: &Path, duration_seconds: u64, outcome: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o666)
        .open(health_dir.join("heartbeat.log"))?;
    writeln!(
        file,
        "{} duration={}s outcome={outcome}",
        Local::now().format("%Y-%m-%dT%H:%M:%S"),
        duration_seconds
    )
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
