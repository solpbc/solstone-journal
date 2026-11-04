// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The last resort for a damaged index. A real read or write failing with a
//! corruption error asks for a check. Only SQLite's own check, finding damage
//! three times with the first two at least ten minutes apart, empties the file.
//! The index is derived, so a rebuild loses nothing, but it costs the owner
//! their older results for a while: nothing short of this path may start one.
//!
//! The `@index-check` token is never claimed and never offered by a snapshot, so
//! only this module writes it. Its `attempts` holds the damaged verdicts so far.
//!
//! | attempts | not_before_ms       | state                  | this drain |
//! |----------|---------------------|------------------------|------------|
//! | 0        | up to 2 h ahead     | retry wait             | carries on |
//! | 0        | due, or further     | first check            | checks     |
//! | 1 or more| up to 20 min ahead  | waiting to check again | holds      |
//! | 1 or more| due                 | second check           | checks     |
//! | 1 or more| further ahead       | a clock moved          | starts over|
//!
//! Further ahead than any wait this module sets means a clock moved. It must not
//! hold saves, put off a check, or stand in for the ten minutes.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use solstone_core_indexer_todo::{Entry, Token, enqueue_token, read_entry, rewrite, root};
use solstone_core_journal_io::append::append_jsonl;
use solstone_core_journal_io::atomic::{AtomicWriteOptions, atomic_replace};
use solstone_core_journal_io::durability::{ArtifactId, DurableRead, artifact, read_json_durable};
use solstone_core_journal_io::whole_journal_restore::{
    WholeJournalRestoreAdmission, whole_journal_restore_admission,
};

use crate::StoreError;
use crate::db::db_path;
use crate::writer_admission::{IndexAdmission, check_test_seam};

const CONFIRM_AFTER_MS: u64 = 10 * 60 * 1000;
const RETRY_MS: u64 = 60 * 60 * 1000;
const CLEAN_REMEMBERED_MS: u64 = 60 * 60 * 1000;
const REPEAT_WINDOW_MS: u64 = 7 * 24 * 60 * 60 * 1000;
#[cfg(not(test))]
const LEASE_WAIT: Duration = Duration::from_secs(10);
#[cfg(test)]
const LEASE_WAIT: Duration = Duration::from_millis(50);
const VERDICT_POLL: Duration = Duration::from_millis(20);

/// What the drain does after the check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AfterCheck {
    CarryOn,
    /// Nothing more this drain: a damaged file takes no writes while it waits to
    /// be checked again, or the drain was told to stop.
    EndDrain,
}

/// What SQLite's own check says about the file, and what it can be trusted to say.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Verdict {
    Clean,
    Damaged(String),
    /// The check couldn't say: the disk failed a read, the file was busy.
    Inconclusive(String),
    Stopped,
}

/// Remembered across the file being emptied, so it can't live in the index.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct RecoveryState {
    #[serde(default)]
    last_clean_ms: Option<u64>,
    #[serde(default)]
    rebuilds_ms: Vec<u64>,
}

#[cfg(test)]
thread_local! {
    /// Moves this module's clock in tests.
    pub(crate) static TEST_CLOCK_OFFSET_MS: std::cell::Cell<i64> =
        const { std::cell::Cell::new(0) };
    /// Makes the Nth read-back from here fail, as a bad sector would.
    pub(crate) static TEST_READ_FAULT_AT: std::cell::Cell<Option<u32>> =
        const { std::cell::Cell::new(None) };
    /// Makes another writer hold the lease when recovery asks for it.
    pub(crate) static TEST_LEASE_BUSY: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    /// Makes emptying the file fail.
    pub(crate) static TEST_RESET_FAULT: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    /// Makes writing the token fail, as on a full disk.
    pub(crate) static TEST_TOKEN_WRITE_FAULT: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    /// Makes SQLite's check say this, once per entry, instead of being run.
    pub(crate) static TEST_VERDICTS: std::cell::RefCell<std::collections::VecDeque<Verdict>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

fn now_ms() -> u64 {
    let now = solstone_core_indexer_todo::now_ms();
    #[cfg(test)]
    let now = now.saturating_add_signed(TEST_CLOCK_OFFSET_MS.get());
    now
}

/// Ask for the file to be checked. Publishing needs no lease, so a reader may
/// do this. It never fails its caller, whose own error is the one that matters.
pub fn report_index_damage(journal: &Path) {
    if let Err(error) = enqueue_token(journal, Token::IndexCheck) {
        log::warn!(target: "solstone::indexer", "index check not queued: {error}");
    }
}

/// `report_index_damage`, if this error suggests damage.
pub fn report_if_damage(journal: &Path, error: &StoreError) {
    if error.suggests_damage() {
        report_index_damage(journal);
    }
}

/// Whether SQLite's words for a damaged file are in this message. Some paths
/// keep only the text of the error. A false match only starts a check.
#[must_use]
pub fn text_suggests_damage(message: &str) -> bool {
    message.contains("database disk image is malformed")
        || message.contains("malformed database schema")
        || message.contains("file is not a database")
}

/// `report_index_damage`, if this message suggests damage.
pub fn report_if_damage_text(journal: &Path, message: &str) {
    if text_suggests_damage(message) {
        report_index_damage(journal);
    }
}

fn token_path(journal: &Path) -> PathBuf {
    root(journal).join(token_filename())
}

fn token_filename() -> String {
    Entry {
        key: Token::IndexCheck.key().to_owned(),
        kind: Token::IndexCheck.key()[1..].to_owned(),
        enqueued_ms: 0,
        attempts: 0,
        not_before_ms: 0,
    }
    .filename()
}

fn remove_token(journal: &Path) {
    if let Err(error) = fs::remove_file(token_path(journal))
        && error.kind() != io::ErrorKind::NotFound
    {
        log::warn!(target: "solstone::indexer", "index check not cleared: {error}");
    }
}

fn set_token(journal: &Path, entry: &Entry, attempts: u32, not_before_ms: u64) -> io::Result<()> {
    #[cfg(test)]
    if TEST_TOKEN_WRITE_FAULT.get() {
        return Err(io::Error::other("no space left on device"));
    }
    let mut entry = entry.clone();
    entry.attempts = attempts;
    entry.not_before_ms = not_before_ms;
    rewrite(&token_path(journal), &entry)
}

fn state_path(journal: &Path) -> PathBuf {
    journal.join(artifact(ArtifactId::IndexRecovery).path)
}

fn read_state(journal: &Path) -> RecoveryState {
    match read_json_durable::<RecoveryState>(ArtifactId::IndexRecovery, &state_path(journal)) {
        Ok(DurableRead::Present(state)) => state,
        _ => RecoveryState::default(),
    }
}

fn write_state(journal: &Path, state: &RecoveryState) {
    let path = state_path(journal);
    let written = serde_json::to_vec(state)
        .map_err(io::Error::other)
        .and_then(|bytes| {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            atomic_replace(&path, &bytes, AtomicWriteOptions::default())
                .map_err(|error| io::Error::other(error.to_string()))
        });
    // Losing this costs one extra check, or one missed line about the disk.
    if let Err(error) = written {
        log::warn!(target: "solstone::indexer", "index recovery state not saved: {error}");
    }
}

/// Whether search has had to rebuild itself twice inside a week.
#[must_use]
pub fn rebuilt_more_than_once_this_week(journal: &Path) -> bool {
    let now = solstone_core_indexer_todo::now_ms();
    read_state(journal)
        .rebuilds_ms
        .iter()
        .filter(|at| now.saturating_sub(**at) < REPEAT_WINDOW_MS)
        .count()
        >= 2
}

/// How SQLite's check row reads. It reports a page it couldn't read as a row,
/// so a failing disk would otherwise look exactly like a damaged index.
pub(crate) fn classify_check_row(row: &str) -> Verdict {
    if row == "ok" {
        return Verdict::Clean;
    }
    if row.contains("failed to get page") {
        return Verdict::Inconclusive(row.to_owned());
    }
    if let Some((_, code)) = row.split_once("error code=") {
        let digits: String = code.chars().take_while(char::is_ascii_digit).collect();
        // The low byte is the primary code, and 11 is SQLITE_CORRUPT.
        if digits.parse::<u32>().is_ok_and(|code| code & 0xff != 11) || digits.is_empty() {
            return Verdict::Inconclusive(row.to_owned());
        }
    }
    Verdict::Damaged(row.to_owned())
}

fn classify_check_error(error: &rusqlite::Error) -> Verdict {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
            Verdict::Damaged(error.to_string())
        }
        _ => Verdict::Inconclusive(error.to_string()),
    }
}

/// SQLite turns a failed disk read into a corruption error in most of its
/// check, so anything that reads as damage has to survive reading the file
/// directly. A bad sector fails again here.
fn file_reads_back(path: &Path, check: &dyn Fn() -> Result<(), String>) -> Result<bool, ()> {
    #[cfg(test)]
    {
        let fault = TEST_READ_FAULT_AT.get();
        if let Some(count) = fault {
            TEST_READ_FAULT_AT.set(count.checked_sub(1));
            if count == 0 {
                return Ok(false);
            }
        }
    }
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    for (candidate, may_be_absent) in [(path.to_path_buf(), false), (PathBuf::from(wal), true)] {
        let mut file = match fs::File::open(&candidate) {
            Ok(file) => file,
            // SQLite removes the WAL with its last connection. The database
            // itself going missing is a drive leaving, not damage.
            Err(error) if may_be_absent && error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Ok(false),
        };
        loop {
            if check().is_err() {
                return Err(());
            }
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Ok(false),
            }
        }
    }
    Ok(true)
}

/// SQLite's own full check, on a fresh read-only connection, stopping when the
/// drain is told to.
pub(crate) fn index_verdict(path: &Path, check: &dyn Fn() -> Result<(), String>) -> Verdict {
    #[cfg(test)]
    if let Some(verdict) = TEST_VERDICTS.with_borrow_mut(std::collections::VecDeque::pop_front) {
        return verdict;
    }
    let connection = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(connection) => connection,
        Err(error) => return classify_check_error(&error),
    };
    if let Err(error) = connection.execute_batch("PRAGMA busy_timeout=5000;") {
        return classify_check_error(&error);
    }
    let interrupt = connection.get_interrupt_handle();
    let mut stopped = false;
    let result = std::thread::scope(|scope| {
        let worker = std::thread::Builder::new().spawn_scoped(scope, move || {
            connection.query_row("PRAGMA integrity_check(1)", [], |row| {
                row.get::<_, String>(0)
            })
        });
        let worker = worker.map_err(|error| error.to_string())?;
        while !worker.is_finished() {
            if !stopped && check().is_err() {
                stopped = true;
                interrupt.interrupt();
            }
            std::thread::sleep(VERDICT_POLL);
        }
        worker
            .join()
            .map_err(|_| "the check stopped unexpectedly".to_owned())
    });
    if stopped {
        return Verdict::Stopped;
    }
    let verdict = match result {
        Ok(Ok(row)) => classify_check_row(&row),
        Ok(Err(error)) => classify_check_error(&error),
        Err(detail) => Verdict::Inconclusive(detail),
    };
    if !matches!(verdict, Verdict::Damaged(_)) {
        return verdict;
    }
    match file_reads_back(path, check) {
        Ok(true) => verdict,
        Ok(false) => Verdict::Inconclusive("the file could not be read back".to_owned()),
        Err(()) => Verdict::Stopped,
    }
}

fn index_is_newer(path: &Path) -> bool {
    if !crate::generation::has_sqlite_header(path) {
        return false;
    }
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(StoreError::from)
        .and_then(|connection| crate::generation::read_stamp(&connection))
        .is_ok_and(|stamp| !stamp.writable())
}

fn wait_an_hour(journal: &Path, entry: &Entry) -> AfterCheck {
    if let Err(error) = set_token(journal, entry, 0, now_ms().saturating_add(RETRY_MS)) {
        log::warn!(target: "solstone::indexer", "index check not rescheduled: {error}");
    }
    AfterCheck::CarryOn
}

fn remember_clean(journal: &Path) {
    let mut state = read_state(journal);
    state.last_clean_ms = Some(now_ms());
    write_state(journal, &state);
}

/// Run the check this drain owes, if any. Called with the drain lock held, after
/// abandoned claims went back on the list.
pub(crate) fn check_index_damage(
    journal: &Path,
    check: &dyn Fn() -> Result<(), String>,
) -> AfterCheck {
    let entry = match read_entry(&token_path(journal), false) {
        Ok(Some(entry)) => entry,
        Ok(None) => return AfterCheck::CarryOn,
        Err(error) => {
            log::warn!(target: "solstone::indexer", "index check unreadable: {error}");
            return AfterCheck::CarryOn;
        }
    };
    let now = now_ms();
    let clock_moved = entry.check_time_is_wrong(now);
    if entry.not_before_ms > now && !clock_moved {
        return if entry.attempts == 0 {
            AfterCheck::CarryOn
        } else {
            AfterCheck::EndDrain
        };
    }
    // An operation keeps failing on a file SQLite calls sound. Checking again on
    // every drain would cost a full read each time and repair nothing.
    if read_state(journal)
        .last_clean_ms
        .is_some_and(|clean| clean <= now && now - clean < CLEAN_REMEMBERED_MS)
    {
        remove_token(journal);
        return AfterCheck::CarryOn;
    }
    let path = db_path(journal);
    match fs::metadata(&path) {
        // Nothing to judge. The normal path builds it.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            remove_token(journal);
            return AfterCheck::CarryOn;
        }
        Ok(metadata) if metadata.len() == 0 => {
            remove_token(journal);
            return AfterCheck::CarryOn;
        }
        Ok(_) => {}
        Err(error) => {
            log::warn!(target: "solstone::indexer", "index check inconclusive: {error}");
            return wait_an_hour(journal, &entry);
        }
    }
    if index_is_newer(&path) {
        remove_token(journal);
        return AfterCheck::CarryOn;
    }
    check_test_seam("recovery-before-verdict");
    match index_verdict(&path, check) {
        Verdict::Clean => {
            remember_clean(journal);
            remove_token(journal);
            return AfterCheck::CarryOn;
        }
        Verdict::Inconclusive(detail) => {
            log::warn!(target: "solstone::indexer", "index check inconclusive: {detail}");
            return wait_an_hour(journal, &entry);
        }
        Verdict::Stopped => return AfterCheck::EndDrain,
        Verdict::Damaged(detail) => {
            log::warn!(target: "solstone::indexer", "index check found damage: {detail}");
        }
    }
    // After a clock moved, what came before can't stand in for the ten minutes.
    if entry.attempts == 0 || clock_moved {
        return match set_token(journal, &entry, 1, now.saturating_add(CONFIRM_AFTER_MS)) {
            Ok(()) => AfterCheck::EndDrain,
            Err(error) => {
                log::warn!(target: "solstone::indexer", "index check not rescheduled: {error}");
                AfterCheck::CarryOn
            }
        };
    }
    recover_if_still_damaged(journal, &entry, &path, check)
}

fn recover_if_still_damaged(
    journal: &Path,
    entry: &Entry,
    path: &Path,
    check: &dyn Fn() -> Result<(), String>,
) -> AfterCheck {
    // Here and not earlier: the verdict only reads, and this walk costs a lot on
    // a large journal. They don't prove the journal is whole. A wrong call here
    // costs time and no data, since the index is derived.
    if whole_journal_restore_admission(journal) != WholeJournalRestoreAdmission::Open {
        log::warn!(target: "solstone::indexer", "index check waiting for a restore");
        return wait_an_hour(journal, entry);
    }
    match solstone_core_indexer::discovery::discover_indexable_files(journal) {
        Ok(files) if !files.is_empty() => {}
        _ => {
            log::warn!(target: "solstone::indexer", "index check found no journal to rebuild from");
            return wait_an_hour(journal, entry);
        }
    }
    #[cfg(test)]
    if TEST_LEASE_BUSY.get() {
        return AfterCheck::CarryOn;
    }
    let admission = match IndexAdmission::acquire_bounded(
        journal,
        "index-recover",
        LEASE_WAIT,
        check,
    ) {
        Ok(admission) => admission,
        // Another writer has it. Nothing changes and nothing is held, and the
        // next drain comes back here.
        Err(StoreError::WriterBusy { .. }) => return AfterCheck::CarryOn,
        Err(StoreError::IndexGenerationNewer { .. }) => {
            remove_token(journal);
            return AfterCheck::CarryOn;
        }
        Err(StoreError::WriterAdmission { .. }) if check().is_err() => {
            return AfterCheck::EndDrain;
        }
        Err(error) => {
            log::warn!(target: "solstone::indexer", "index check could not take the writer: {error}");
            return wait_an_hour(journal, entry);
        }
    };
    if index_is_newer(path) {
        remove_token(journal);
        return AfterCheck::CarryOn;
    }
    match index_verdict(path, check) {
        Verdict::Clean => {
            remember_clean(journal);
            remove_token(journal);
            AfterCheck::CarryOn
        }
        Verdict::Inconclusive(detail) => {
            log::warn!(target: "solstone::indexer", "index check inconclusive: {detail}");
            wait_an_hour(journal, entry)
        }
        Verdict::Stopped => AfterCheck::EndDrain,
        Verdict::Damaged(_) => recover(journal, entry, path, &admission),
    }
}

/// The order is what makes a kill safe: the rebuild is queued before the file is
/// emptied, and the token goes last.
fn recover(journal: &Path, entry: &Entry, path: &Path, admission: &IndexAdmission) -> AfterCheck {
    if let Err(error) = queue_fresh_full_pass(journal) {
        log::warn!(target: "solstone::indexer", "index rebuild not queued: {error}");
        return wait_an_hour(journal, entry);
    }
    record_recovery(journal);
    check_test_seam("recovery-queued");
    #[cfg(test)]
    if TEST_RESET_FAULT.get() {
        return wait_an_hour(journal, entry);
    }
    if let Err(error) = crate::db::empty_damaged_index(path) {
        log::warn!(target: "solstone::indexer", "damaged index not emptied: {error}");
        return wait_an_hour(journal, entry);
    }
    check_test_seam("recovery-emptied");
    if let Err(error) = crate::db::rebuild_empty_index(journal) {
        log::warn!(target: "solstone::indexer", "emptied index not prepared: {error}");
        return wait_an_hour(journal, entry);
    }
    check_test_seam("recovery-prepared");
    // After the reset worked, so one that had to be retried counts once.
    let mut state = read_state(journal);
    state.last_clean_ms = None;
    state.rebuilds_ms.push(now_ms());
    let keep_from = state.rebuilds_ms.len().saturating_sub(2);
    state.rebuilds_ms.drain(..keep_from);
    write_state(journal, &state);
    // Entity rows are in `chunks`, and until any row exists readers call the
    // index empty. A failure stops nothing: the finishing pass writes them.
    if let Err(error) = crate::scan::update_entity_search_admitted(journal, admission) {
        log::warn!(target: "solstone::indexer", "entity search not rebuilt yet: {error}");
    }
    check_test_seam("recovery-searchable");
    remove_token(journal);
    AfterCheck::CarryOn
}

/// `enqueue_token` is create-only, and a full pass may have been failing against
/// this file. Left as it is, it could wait an hour, run as one blocking pass, or
/// read as failed.
fn queue_fresh_full_pass(journal: &Path) -> io::Result<()> {
    enqueue_token(journal, Token::ScanFull)?;
    let name = Entry {
        key: Token::ScanFull.key().to_owned(),
        kind: Token::ScanFull.key()[1..].to_owned(),
        enqueued_ms: 0,
        attempts: 0,
        not_before_ms: 0,
    }
    .filename();
    let path = root(journal).join(name);
    if let Some(mut entry) = read_entry(&path, false)? {
        entry.attempts = 0;
        entry.not_before_ms = 0;
        rewrite(&path, &entry)?;
    }
    Ok(())
}

fn record_recovery(journal: &Path) {
    let now = chrono::Utc::now();
    let log_path = journal.join(artifact(ArtifactId::IndexAttempts).path.replacen(
        '*',
        &now.format("%Y%m%d").to_string(),
        1,
    ));
    let event = serde_json::json!({"event": "index.recovery", "ts": now.timestamp_millis()});
    if let Err(error) = append_jsonl(&log_path, &event) {
        log::warn!(target: "solstone::indexer", "index recovery not logged: {error}");
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "recovery_tests.rs"]
mod tests;
