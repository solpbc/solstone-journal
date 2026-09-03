// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use chrono::{DateTime, FixedOffset, Local, Utc};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use serde_json::json;
use solstone_core_journal_io::{
    JournalRoot, MalformedPolicy,
    operational_log::{OplogFormat, OplogWriter, catalog_oplogs, create_oplog_at},
    read_jsonl_with_report,
};

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

    if !force && recently_succeeded(journal, Local::now().fixed_offset())? {
        return Ok(());
    }

    let pid_path = health_dir.join("heartbeat.pid");
    let result = guarded_pass(journal, &pid_path);
    // The reference unlinks from its outer finally block on every guarded path.
    let cleanup = remove_if_present(&pid_path);
    result.and(cleanup)
}

fn recently_succeeded(journal: &Path, now: DateTime<FixedOffset>) -> io::Result<bool> {
    let today = now.date_naive();
    let Some(previous) = today.pred_opt() else {
        return Ok(false);
    };

    let snapshot = match JournalRoot::open(journal)
        .ok()
        .and_then(|root| catalog_oplogs(root, &[previous, today]).ok())
    {
        Some(snapshot) => snapshot,
        None => return Ok(false),
    };
    let mut newest_success = None;
    for entry in snapshot.entries() {
        if entry.name().source().display_slug() != "heartbeat"
            || entry.name().run().display_slug() != "pass"
            || entry.name().format() != OplogFormat::Jsonl
        {
            continue;
        }

        let path = journal
            .join("chronicle")
            .join(entry.day())
            .join("health")
            .join(entry.leaf());
        let Ok(report) =
            read_jsonl_with_report::<serde_json::Value>(&path, Vec::new(), MalformedPolicy::Skip)
        else {
            continue;
        };
        for record in report.records {
            let serde_json::Value::Object(row) = record.value else {
                continue;
            };
            if row.get("event").and_then(serde_json::Value::as_str) != Some("pass.outcome")
                || row.get("outcome").and_then(serde_json::Value::as_str) != Some("success")
                || row
                    .get("duration_seconds")
                    .and_then(serde_json::Value::as_u64)
                    .is_none()
            {
                continue;
            }
            let Some(ts) = row.get("ts").and_then(serde_json::Value::as_i64) else {
                continue;
            };
            newest_success = Some(newest_success.map_or(ts, |current: i64| current.max(ts)));
        }
    }

    Ok(newest_success.is_some_and(|ts| {
        now.timestamp_millis().saturating_sub(ts) < RECENCY_WINDOW_HOURS * 3_600_000
    }))
}

fn guarded_pass(journal: &Path, pid_path: &Path) -> io::Result<()> {
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
    let mut log = open_heartbeat_log(journal, Local::now().fixed_offset())?;
    let pass = run_pass(&mut log);
    let outcome = if pass.is_ok() { "success" } else { "error" };
    let log_result = append_heartbeat_log(&mut log, started.elapsed().as_secs(), outcome);
    pass.and(log_result)
}

fn open_heartbeat_log(journal: &Path, opened: DateTime<FixedOffset>) -> io::Result<OplogWriter> {
    let root = JournalRoot::open(journal).map_err(|error| io::Error::other(error.to_string()))?;
    create_oplog_at(root, "heartbeat", "pass", OplogFormat::Jsonl, opened)
        .map_err(|error| io::Error::other(format!("failed to create heartbeat oplog: {error}")))
}

fn run_pass(log: &mut OplogWriter) -> io::Result<()> {
    let record = json!({
        "data_source_errors": [],
        "escalated_targets": [],
        "event": "pass",
        "fired": [],
        "ts": Utc::now().timestamp_millis(),
    });
    write_record(log, &record)
}

fn append_heartbeat_log(
    log: &mut OplogWriter,
    duration_seconds: u64,
    outcome: &str,
) -> io::Result<()> {
    write_record(
        log,
        &json!({
            "duration_seconds": duration_seconds,
            "event": "pass.outcome",
            "outcome": outcome,
            "ts": Utc::now().timestamp_millis(),
        }),
    )
}

fn write_record(log: &mut OplogWriter, record: &serde_json::Value) -> io::Result<()> {
    serde_json::to_writer(&mut *log, record).map_err(io::Error::other)?;
    log.write_all(b"\n")?;
    log.flush()
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
