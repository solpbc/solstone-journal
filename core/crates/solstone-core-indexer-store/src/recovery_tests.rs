// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use super::*;
use crate::db::open_index;
use crate::generation::read_last_scan;
use crate::scan::scan_journal;
use crate::todo::{DrainOptions, drain, enqueue_path};
use std::io::{Seek, SeekFrom, Write};

const NOTES: usize = 120;
const TEN_MINUTES: i64 = 10 * 60 * 1000;
const HOUR: i64 = 60 * 60 * 1000;

struct Journal(PathBuf);
impl Journal {
    fn empty() -> Self {
        TEST_CLOCK_OFFSET_MS.set(0);
        TEST_READ_FAULT_AT.set(None);
        TEST_RESET_FAULT.set(false);
        TEST_LEASE_BUSY.set(false);
        TEST_TOKEN_WRITE_FAULT.set(false);
        TEST_VERDICTS.with_borrow_mut(std::collections::VecDeque::clear);
        let root = crate::test_support::reserve_temp_path("index-recovery");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
        Self(root)
    }
    /// A journal with a complete index, checkpointed so damage reaches its rows.
    fn indexed() -> Self {
        let journal = Self::empty();
        for index in 0..NOTES {
            journal.save(
                &note(index),
                &format!("kestrel note {index} {}", "word ".repeat(60)),
            );
        }
        scan_journal(journal.path(), true).unwrap();
        journal.checkpoint();
        journal
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn db(&self) -> PathBuf {
        db_path(self.path())
    }
    fn checkpoint(&self) {
        Connection::open(self.db())
            .unwrap()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    }
    fn save(&self, rel: &str, text: &str) -> PathBuf {
        let path = solstone_core_format::paths::resolve_journal_path(self.path(), rel).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
    fn token(&self) -> Option<Entry> {
        read_entry(&token_path(self.path()), false).unwrap()
    }
    fn set_token(&self, attempts: u32, not_before_ms: u64) {
        report_index_damage(self.path());
        let entry = self.token().unwrap();
        set_token(self.path(), &entry, attempts, not_before_ms).unwrap();
    }
    fn kestrels(&self) -> i64 {
        Connection::open_with_flags(self.db(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM chunks WHERE chunks MATCH 'kestrel'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }
    fn pages(&self) -> u64 {
        fs::metadata(self.db()).unwrap().len() / 4096
    }
    fn root_page(&self, name: &str) -> u64 {
        Connection::open_with_flags(self.db(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name=?",
                [name],
                |row| row.get::<_, i64>(0),
            )
            .unwrap() as u64
    }
    fn scribble(&self, pages: &[u64]) {
        let mut file = fs::OpenOptions::new().write(true).open(self.db()).unwrap();
        let noise: Vec<u8> = (0..4096u32)
            .map(|byte| (byte.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        for page in pages {
            file.seek(SeekFrom::Start(page * 4096)).unwrap();
            file.write_all(&noise).unwrap();
        }
    }
    /// Run drains until nothing is left: the check, the turns, the finish.
    fn drain_until_settled(&self) {
        for _ in 0..50 {
            drain(self.path(), DrainOptions::default()).unwrap();
            let status = crate::todo::inspect(self.path()).unwrap();
            if status.keys.is_empty() && status.claimed == 0 && !status.index_check {
                return;
            }
        }
        panic!("the journal never settled");
    }
    fn assert_rebuilt(&self) {
        self.drain_until_settled();
        assert_eq!(self.kestrels(), NOTES as i64);
        let conn = open_index(self.path()).unwrap();
        let check: String = conn
            .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        let state = crate::db::read_index_build_state(&conn).unwrap().unwrap();
        assert_eq!(state.state, crate::db::IndexBuildLifecycle::Complete);
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        TEST_CLOCK_OFFSET_MS.set(0);
        TEST_READ_FAULT_AT.set(None);
        TEST_RESET_FAULT.set(false);
        TEST_LEASE_BUSY.set(false);
        TEST_TOKEN_WRITE_FAULT.set(false);
        TEST_VERDICTS.with_borrow_mut(std::collections::VecDeque::clear);
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn note(index: usize) -> String {
    let day =
        chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(index as i64);
    format!(
        "{}/desktop/120000_300/talents/activity.md",
        day.format("%Y%m%d")
    )
}

fn verdicts(list: &[Verdict]) {
    TEST_VERDICTS.with_borrow_mut(|queue| queue.extend(list.iter().cloned()));
}

fn verdicts_left() -> usize {
    TEST_VERDICTS.with_borrow(std::collections::VecDeque::len)
}

fn damaged() -> Verdict {
    Verdict::Damaged("test damage".to_owned())
}

fn advance(ms: i64) {
    TEST_CLOCK_OFFSET_MS.set(TEST_CLOCK_OFFSET_MS.get() + ms);
}

/// Run the whole path by hand: a failure reported, then both checks.
fn confirm_and_recover(journal: &Journal) {
    report_index_damage(journal.path());
    let first = drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(first.timed_out, "the first check holds the drain");
    assert_eq!(journal.token().unwrap().attempts, 1);
    advance(TEN_MINUTES + 1);
    drain(journal.path(), DrainOptions::default()).unwrap();
}

#[test]
fn a_healthy_journal_reads_the_token_file_and_nothing_else() {
    let journal = Journal::indexed();
    verdicts(&[damaged()]);
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(!report.timed_out);
    assert_eq!(verdicts_left(), 1, "no token, so no check");
    assert!(!state_path(journal.path()).exists());
    assert_eq!(journal.kestrels(), NOTES as i64);
}

#[test]
fn a_forced_drain_leaves_a_waiting_check_alone() {
    let journal = Journal::indexed();
    // In a retry wait. A forced drain ignores delays on work, so it must not
    // see this as work: nothing could process it.
    journal.set_token(0, now_ms() + HOUR as u64);
    verdicts(&[damaged()]);
    let source = journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    enqueue_path(journal.path(), &source).unwrap();
    let report = drain(
        journal.path(),
        DrainOptions {
            force: true,
            ..DrainOptions::default()
        },
    )
    .unwrap();
    assert_eq!((report.processed, report.failed), (1, 0));
    assert_eq!(verdicts_left(), 1);
    assert_eq!(journal.token().unwrap().attempts, 0);
}

type Damage = fn(&Journal);

#[test]
fn every_damage_shape_is_emptied_after_three_verdicts_and_rebuilt() {
    let shapes: [(&str, Damage); 9] = [
        ("schema", |journal| {
            let conn = Connection::open(journal.db()).unwrap();
            conn.execute_batch(
                "PRAGMA writable_schema=ON;
                 UPDATE sqlite_master SET sql='not sql at all' WHERE name='files';
                 PRAGMA writable_schema=OFF;
                 PRAGMA wal_checkpoint(TRUNCATE);",
            )
            .unwrap();
        }),
        ("header", |journal| journal.scribble(&[0])),
        ("truncated", |journal| {
            let file = fs::OpenOptions::new()
                .write(true)
                .open(journal.db())
                .unwrap();
            file.set_len(journal.pages() / 2 * 4096).unwrap();
        }),
        ("page one", |journal| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .open(journal.db())
                .unwrap();
            file.seek(SeekFrom::Start(100)).unwrap();
            file.write_all(&[0x5a; 3900]).unwrap();
        }),
        ("many pages", |journal| {
            let pages: Vec<u64> = (2..journal.pages()).step_by(20).collect();
            journal.scribble(&pages);
        }),
        ("fts root", |journal| {
            journal.scribble(&[journal.root_page("chunks_data") - 1]);
        }),
        ("fts blobs", |journal| {
            let conn = Connection::open(journal.db()).unwrap();
            conn.execute_batch(
                "UPDATE chunks_data SET block=randomblob(length(block)) WHERE id>10;",
            )
            .unwrap();
            drop(conn);
            journal.checkpoint();
        }),
        ("files root", |journal| {
            journal.scribble(&[journal.root_page("files") - 1]);
        }),
        ("one leaf", |journal| {
            journal.scribble(&[journal.pages() * 3 / 4]);
        }),
    ];
    for (name, damage) in shapes {
        let journal = Journal::indexed();
        damage(&journal);
        confirm_and_recover(&journal);
        assert!(journal.token().is_none(), "{name}");
        journal.assert_rebuilt();
    }
}

/// A flipped key in a valid index page: `quick_check` reads clean, and real
/// lookups can still fail.
#[test]
fn damage_only_a_full_check_sees_is_still_damage() {
    let journal = Journal::indexed();
    let page = journal.root_page("chunk_sources_path") - 1;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(journal.db())
        .unwrap();
    file.seek(SeekFrom::Start(page * 4096 + 4096 - 16)).unwrap();
    file.write_all(&[0x7a; 8]).unwrap();
    drop(file);
    let verdict = index_verdict(&journal.db(), &|| Ok(()));
    assert!(matches!(verdict, Verdict::Damaged(_)), "{verdict:?}");
    confirm_and_recover(&journal);
    journal.assert_rebuilt();
}

#[test]
fn a_garbage_wal_and_a_sound_file_are_never_reset() {
    for garbage_wal in [false, true] {
        let journal = Journal::indexed();
        if garbage_wal {
            let mut wal = journal.db().into_os_string();
            wal.push("-wal");
            fs::write(wal, [0x5a; 8192]).unwrap();
        }
        report_index_damage(journal.path());
        let report = drain(journal.path(), DrainOptions::default()).unwrap();
        assert!(!report.timed_out);
        assert!(journal.token().is_none());
        assert_eq!(journal.kestrels(), NOTES as i64);
        assert!(read_state(journal.path()).last_clean_ms.is_some());
        assert!(read_state(journal.path()).rebuilds_ms.is_empty());
    }
}

#[test]
fn a_zero_byte_file_is_left_to_the_normal_build() {
    let journal = Journal::indexed();
    fs::write(journal.db(), b"").unwrap();
    verdicts(&[damaged()]);
    report_index_damage(journal.path());
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(journal.token().is_none());
    assert_eq!(verdicts_left(), 1, "nothing to judge");
    assert!(read_state(journal.path()).rebuilds_ms.is_empty());
}

#[test]
fn damage_no_operation_hit_starts_nothing() {
    let journal = Journal::indexed();
    journal.scribble(&[journal.pages() * 3 / 4]);
    let before = fs::read(journal.db()).unwrap();
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(!report.timed_out);
    assert!(journal.token().is_none());
    assert_eq!(before, fs::read(journal.db()).unwrap());
}

#[test]
fn three_verdicts_are_needed_and_ten_minutes_separate_the_first_two() {
    let journal = Journal::indexed();
    journal.scribble(&[0]);
    let source = journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    report_index_damage(journal.path());
    enqueue_path(journal.path(), &source).unwrap();
    let forced = DrainOptions {
        force: true,
        ..DrainOptions::default()
    };
    for (advance_by, attempts) in [(0, 1), (0, 1), (TEN_MINUTES - 2000, 1)] {
        advance(advance_by);
        let report = drain(journal.path(), forced).unwrap();
        assert!(report.timed_out, "a held drain says saves are waiting");
        assert_eq!(report.processed, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(journal.token().unwrap().attempts, attempts);
    }
    // The save is still on the list, with no attempt spent.
    let queued = crate::todo::inspect(journal.path()).unwrap();
    assert!(queued.keys.iter().any(|key| key.ends_with("activity.md")));
    assert!(
        solstone_core_indexer_todo::entry_paths(&root(journal.path()))
            .unwrap()
            .iter()
            .filter_map(|path| read_entry(path, false).unwrap())
            .all(|entry| entry.attempts == 0 || entry.key == "@index-check")
    );
    advance(3000);
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(journal.token().is_none());
    assert_eq!(read_state(journal.path()).rebuilds_ms.len(), 1);
}

#[test]
fn a_clock_that_moved_cannot_hold_saves_or_silence_a_check() {
    let journal = Journal::indexed();
    // Waiting for its second check, with a time no wait could set. It checks, and
    // what it finds can't count as the second: the ten minutes didn't pass.
    journal.set_token(1, now_ms() + 24 * HOUR as u64);
    let token = journal.token().unwrap();
    assert!(token.check_time_is_wrong(now_ms()));
    assert!(solstone_core_indexer_todo::has_due(journal.path(), now_ms()).unwrap());
    verdicts(&[damaged()]);
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(report.timed_out, "held for a real ten minutes");
    assert_eq!(verdicts_left(), 0, "it checked");
    assert_eq!(journal.token().unwrap().attempts, 1);
    assert!(read_state(journal.path()).rebuilds_ms.is_empty());
    assert!(journal.token().unwrap().not_before_ms <= now_ms() + TEN_MINUTES as u64);
    remove_token(journal.path());
    journal.set_token(1, now_ms() + 24 * HOUR as u64);
    verdicts(&[Verdict::Clean]);
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(!report.timed_out);
    assert!(journal.token().is_none());

    // So can a retry wait with a time no wait could set.
    fs::remove_file(state_path(journal.path())).unwrap();
    journal.set_token(0, now_ms() + 24 * HOUR as u64);
    verdicts(&[Verdict::Clean]);
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(verdicts_left(), 0, "it checked");
    assert!(journal.token().is_none());
    fs::remove_file(state_path(journal.path())).unwrap();

    // A clean check recorded by a clock that was ahead suppresses nothing.
    write_state(
        journal.path(),
        &RecoveryState {
            last_clean_ms: Some(now_ms() + 24 * HOUR as u64),
            rebuilds_ms: Vec::new(),
        },
    );
    report_index_damage(journal.path());
    verdicts(&[Verdict::Clean]);
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(verdicts_left(), 0, "it checked");
}

#[test]
fn a_clean_verdict_at_any_step_ends_it() {
    for list in [
        vec![Verdict::Clean],
        vec![damaged(), Verdict::Clean],
        vec![damaged(), damaged(), Verdict::Clean],
    ] {
        let journal = Journal::indexed();
        let rounds = list.len();
        verdicts(&list);
        report_index_damage(journal.path());
        for _ in 0..rounds {
            drain(journal.path(), DrainOptions::default()).unwrap();
            advance(TEN_MINUTES + 1);
        }
        assert!(journal.token().is_none());
        assert_eq!(journal.kestrels(), NOTES as i64);
        assert!(read_state(journal.path()).rebuilds_ms.is_empty());
        assert!(read_state(journal.path()).last_clean_ms.is_some());
        TEST_CLOCK_OFFSET_MS.set(0);
    }
}

fn inconclusive() -> Verdict {
    Verdict::Inconclusive("test".to_owned())
}

#[test]
fn an_inconclusive_verdict_counts_for_nothing_and_clears_what_came_before() {
    let journal = Journal::indexed();
    verdicts(&[damaged(), inconclusive()]);
    report_index_damage(journal.path());
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(journal.token().unwrap().attempts, 1);
    advance(TEN_MINUTES + 1);
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(!report.timed_out, "no hold while we don't know");
    let token = journal.token().unwrap();
    assert_eq!(token.attempts, 0);
    assert!(token.not_before_ms > now_ms() + (HOUR as u64) / 2);
    assert_eq!(journal.kestrels(), NOTES as i64);
}

#[test]
fn a_disk_that_fails_reads_never_counts_as_damage_at_any_of_the_three_checks() {
    // Each of the three: the fault lands on that check's read of the file.
    for faulted in 0..3_u32 {
        let journal = Journal::indexed();
        journal.scribble(&[0]);
        report_index_damage(journal.path());
        TEST_READ_FAULT_AT.set(Some(faulted));
        // First check, then the second and third together, ten minutes later.
        let _ = drain(journal.path(), DrainOptions::default());
        advance(TEN_MINUTES + 1);
        let _ = drain(
            journal.path(),
            DrainOptions {
                force: false,
                budget: Duration::from_secs(20),
            },
        );
        let token = journal.token().unwrap_or_else(|| panic!("check {faulted}"));
        // Inconclusive at whichever check it hit, so the count started over.
        assert_eq!(token.attempts, 0, "check {faulted}");
        assert!(read_state(journal.path()).rebuilds_ms.is_empty());
    }
}

#[test]
fn the_real_read_back_needs_the_database_and_not_its_wal() {
    let journal = Journal::indexed();
    let mut wal = journal.db().into_os_string();
    wal.push("-wal");
    let _ = fs::remove_file(&wal);
    assert_eq!(file_reads_back(&journal.db(), &|| Ok(())), Ok(true));
    assert_eq!(
        file_reads_back(&journal.path().join("gone.sqlite"), &|| Ok(())),
        Ok(false)
    );
    assert_eq!(
        file_reads_back(&journal.db(), &|| Err("stopped".to_owned())),
        Err(())
    );
}

#[test]
fn the_check_treats_only_corruption_as_damage_when_it_fails_outright() {
    use rusqlite::ffi;
    let failure = |code| rusqlite::Error::SqliteFailure(ffi::Error::new(code), None);
    for code in [
        ffi::SQLITE_CORRUPT,
        ffi::SQLITE_NOTADB,
        ffi::SQLITE_CORRUPT_VTAB,
    ] {
        assert!(
            matches!(classify_check_error(&failure(code)), Verdict::Damaged(_)),
            "{code}"
        );
    }
    for code in [
        ffi::SQLITE_BUSY,
        ffi::SQLITE_LOCKED,
        ffi::SQLITE_IOERR,
        ffi::SQLITE_IOERR_READ,
        ffi::SQLITE_READONLY,
        ffi::SQLITE_CANTOPEN,
        ffi::SQLITE_PERM,
        ffi::SQLITE_FULL,
        ffi::SQLITE_NOMEM,
        ffi::SQLITE_INTERRUPT,
    ] {
        assert!(
            matches!(
                classify_check_error(&failure(code)),
                Verdict::Inconclusive(_)
            ),
            "{code}"
        );
    }
}

#[test]
fn the_check_reads_its_rows_correctly() {
    let page = "*** in database main ***\nTree 7 page 7: unable to get the page. error code=";
    let classify = |row: String| classify_check_row(&row);
    assert_eq!(classify("ok".to_owned()), Verdict::Clean);
    assert!(matches!(classify(format!("{page}11")), Verdict::Damaged(_)));
    // SQLITE_CORRUPT_PAGE and friends share the primary code.
    assert!(matches!(
        classify(format!("{page}779")),
        Verdict::Damaged(_)
    ));
    for code in ["266", "10", "8458", "778"] {
        assert!(
            matches!(classify(format!("{page}{code}")), Verdict::Inconclusive(_)),
            "{code}"
        );
    }
    assert!(matches!(
        classify("*** in database main ***\nFreelist: failed to get page 9".to_owned()),
        Verdict::Inconclusive(_)
    ));
    assert!(matches!(
        classify(
            "*** in database main ***\nTree 4 page 4: btreeInitPage() returns error code 11"
                .to_owned()
        ),
        Verdict::Damaged(_)
    ));
    assert!(matches!(
        classify("row 12 missing from index chunk_sources_path".to_owned()),
        Verdict::Damaged(_)
    ));
    assert!(matches!(
        classify("malformed inverted index for FTS5 table main.chunks".to_owned()),
        Verdict::Damaged(_)
    ));
}

#[test]
fn a_journal_that_looks_gone_is_not_rebuilt_from_nothing() {
    let journal = Journal::indexed();
    let chronicle = journal.path().join("chronicle");
    let moved = journal.path().join("elsewhere");
    fs::rename(&chronicle, &moved).unwrap();
    verdicts(&[damaged(), damaged()]);
    report_index_damage(journal.path());
    drain(journal.path(), DrainOptions::default()).unwrap();
    advance(TEN_MINUTES + 1);
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(!report.timed_out);
    assert_eq!(journal.token().unwrap().attempts, 0);
    assert_eq!(journal.kestrels(), NOTES as i64);
    assert!(read_state(journal.path()).rebuilds_ms.is_empty());
}

// A restore record can't be published on Windows, so none can be pending there.
#[cfg(unix)]
#[test]
fn a_pending_restore_is_waited_for() {
    let journal = Journal::indexed();
    let source = solstone_core_journal_io::whole_journal_restore::WholeJournalRestoreSource::Byo {
        repository: "repo".into(),
        backend: "s3".into(),
    };
    solstone_core_journal_io::whole_journal_restore::publish_whole_journal_restore(
        journal.path(),
        &source,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "/journal",
    )
    .unwrap();
    verdicts(&[damaged(), damaged()]);
    report_index_damage(journal.path());
    drain(journal.path(), DrainOptions::default()).unwrap();
    advance(TEN_MINUTES + 1);
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(journal.token().unwrap().attempts, 0);
    assert_eq!(journal.kestrels(), NOTES as i64);
}

#[test]
fn a_newer_index_is_not_this_binarys_to_touch() {
    let journal = Journal::indexed();
    Connection::open(journal.db())
        .unwrap()
        .execute_batch("PRAGMA user_version=99;")
        .unwrap();
    verdicts(&[damaged()]);
    report_index_damage(journal.path());
    let _ = drain(journal.path(), DrainOptions::default());
    assert!(journal.token().is_none());
    assert_eq!(verdicts_left(), 1);
    assert!(read_state(journal.path()).rebuilds_ms.is_empty());
}

#[test]
fn a_lease_another_writer_really_holds_changes_nothing() {
    let journal = Journal::indexed();
    journal.set_token(1, 0);
    let token_before = journal.token().unwrap();
    verdicts(&[damaged()]);
    let lease =
        solstone_core_journal_io::locking::hold_lock(db_path(journal.path()), Default::default())
            .unwrap();
    let report = drain(
        journal.path(),
        DrainOptions {
            force: false,
            budget: Duration::from_millis(500),
        },
    )
    .unwrap();
    drop(lease);
    assert_eq!(report.failed, 0);
    let token = journal.token().unwrap();
    assert_eq!(token.attempts, token_before.attempts);
    assert_eq!(token.not_before_ms, token_before.not_before_ms);
    assert_eq!(journal.kestrels(), NOTES as i64);
    assert!(read_state(journal.path()).rebuilds_ms.is_empty());
}

#[test]
fn a_busy_writer_changes_nothing_and_holds_nothing() {
    let journal = Journal::indexed();
    // First check, one before the busy lease, then two when it's free.
    verdicts(&[damaged(), damaged(), damaged(), damaged()]);
    report_index_damage(journal.path());
    drain(journal.path(), DrainOptions::default()).unwrap();
    advance(TEN_MINUTES + 1);
    let source = journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    enqueue_path(journal.path(), &source).unwrap();
    let token_before = journal.token().unwrap();
    TEST_LEASE_BUSY.set(true);
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    TEST_LEASE_BUSY.set(false);
    assert!(!report.timed_out, "nothing is held for a busy writer");
    assert_eq!(
        (report.processed, report.failed),
        (1, 0),
        "the save was served"
    );
    let token = journal.token().unwrap();
    assert_eq!(token.attempts, token_before.attempts);
    assert_eq!(token.not_before_ms, token_before.not_before_ms);
    assert!(read_state(journal.path()).rebuilds_ms.is_empty());
    // Free again: the next drain carries on and recovers.
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(journal.token().is_none());
    assert_eq!(read_state(journal.path()).rebuilds_ms.len(), 1);
}

#[test]
fn a_reset_that_keeps_failing_holds_at_most_ten_minutes_an_hour() {
    let journal = Journal::indexed();
    let source = journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    TEST_RESET_FAULT.set(true);
    report_index_damage(journal.path());
    for _ in 0..3 {
        // One more than the cycle needs, to show the hour runs no check.
        verdicts(&[damaged(), damaged(), damaged(), damaged()]);
        // The first check holds, for ten minutes at most.
        assert!(
            drain(journal.path(), DrainOptions::default())
                .unwrap()
                .timed_out
        );
        assert_eq!(journal.token().unwrap().attempts, 1);
        advance(TEN_MINUTES + 1);
        // The reset fails, and that drain carries on with a save in hand.
        enqueue_path(journal.path(), &source).unwrap();
        let report = drain(journal.path(), DrainOptions::default()).unwrap();
        assert!(!report.timed_out);
        assert!(report.processed >= 1, "work runs after a failed reset");
        assert_eq!(journal.token().unwrap().attempts, 0);
        // Nothing is held for the hour after it.
        enqueue_path(journal.path(), &source).unwrap();
        let report = drain(journal.path(), DrainOptions::default()).unwrap();
        assert!(!report.timed_out);
        assert_eq!(verdicts_left(), 1, "and it doesn't check again yet");
        TEST_VERDICTS.with_borrow_mut(std::collections::VecDeque::clear);
        advance(HOUR + 1);
    }
    assert!(read_state(journal.path()).rebuilds_ms.is_empty());
    assert_eq!(journal.kestrels(), NOTES as i64 + 1);
}

#[test]
fn an_operation_failing_on_a_sound_file_checks_once_an_hour() {
    let journal = Journal::indexed();
    report_index_damage(journal.path());
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(journal.token().is_none());
    // The same failure again, on every drain, doesn't read the file again.
    verdicts(&[damaged()]);
    for _ in 0..3 {
        report_index_damage(journal.path());
        drain(journal.path(), DrainOptions::default()).unwrap();
        assert!(journal.token().is_none());
    }
    assert_eq!(verdicts_left(), 1);
    advance(HOUR + 1);
    report_index_damage(journal.path());
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(verdicts_left(), 0, "an hour on, it checks");
}

#[test]
fn a_full_pass_that_had_been_failing_starts_fresh_after_recovery() {
    let journal = Journal::indexed();
    journal.scribble(&[0]);
    enqueue_token(journal.path(), Token::ScanFull).unwrap();
    let name = solstone_core_indexer_todo::entry_paths(&root(journal.path()))
        .unwrap()
        .into_iter()
        .find(|path| {
            read_entry(path, false)
                .unwrap()
                .is_some_and(|entry| entry.key == "@scan-full")
        })
        .unwrap();
    let mut stale = read_entry(&name, false).unwrap().unwrap();
    stale.attempts = 3;
    stale.not_before_ms = now_ms() + HOUR as u64;
    rewrite(&name, &stale).unwrap();
    confirm_and_recover(&journal);
    // That drain went on to run it, or left it due and unmarked.
    match read_entry(&name, false).unwrap() {
        None => {}
        Some(entry) => assert_eq!((entry.attempts, entry.not_before_ms), (0, 0)),
    }
    journal.assert_rebuilt();
    let status = crate::todo::inspect(journal.path()).unwrap();
    assert_eq!(status.failed_tokens, 0);
}

#[test]
fn a_claimed_full_pass_goes_back_before_recovery_refreshes_it() {
    let journal = Journal::indexed();
    journal.scribble(&[0]);
    enqueue_token(journal.path(), Token::ScanFull).unwrap();
    let listed = solstone_core_indexer_todo::entry_paths(&root(journal.path()))
        .unwrap()
        .into_iter()
        .find(|path| {
            read_entry(path, false)
                .unwrap()
                .is_some_and(|entry| entry.key == "@scan-full")
        })
        .unwrap();
    let claimed = root(journal.path()).join("claimed");
    fs::create_dir_all(&claimed).unwrap();
    fs::rename(&listed, claimed.join(listed.file_name().unwrap())).unwrap();
    confirm_and_recover(&journal);
    journal.assert_rebuilt();
    assert_eq!(
        crate::todo::inspect(journal.path()).unwrap().failed_tokens,
        0
    );
}

#[test]
fn a_writer_failure_reaches_the_check() {
    // The lease fails before any entry runs: the entry is untouched, so only the
    // drain's own return can have raised it.
    let journal = Journal::indexed();
    let file = fs::OpenOptions::new()
        .write(true)
        .open(journal.db())
        .unwrap();
    file.set_len(journal.pages() / 2 * 4096).unwrap();
    let source = journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    enqueue_path(journal.path(), &source).unwrap();
    assert!(drain(journal.path(), DrainOptions::default()).is_err());
    assert!(journal.token().is_some(), "from the lease");
    assert!(
        solstone_core_indexer_todo::entry_paths(&root(journal.path()))
            .unwrap()
            .iter()
            .filter_map(|path| read_entry(path, false).unwrap())
            .all(|entry| entry.attempts == 0)
    );

    // An entry fails.
    let journal = Journal::indexed();
    journal.scribble(&[0]);
    let source = journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    enqueue_path(journal.path(), &source).unwrap();
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(report.failed, 1);
    assert!(journal.token().is_some(), "from an entry");

    // The step that invalidates a path after two failures, before its entry runs.
    let journal = Journal::indexed();
    journal.scribble(&[0]);
    let source = journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    enqueue_path(journal.path(), &source).unwrap();
    let listed = solstone_core_indexer_todo::entry_paths(&root(journal.path()))
        .unwrap()
        .pop()
        .unwrap();
    let mut entry = read_entry(&listed, false).unwrap().unwrap();
    entry.attempts = 2;
    rewrite(&listed, &entry).unwrap();
    assert!(drain(journal.path(), DrainOptions::default()).is_err());
    assert!(journal.token().is_some(), "from invalidating a path");

    // A scan, directly.
    let journal = Journal::indexed();
    journal.scribble(&[0]);
    assert!(scan_journal(journal.path(), false).is_err());
    assert!(journal.token().is_some(), "from a scan");
}

/// SQLite's own words raised from inside an insert, which is how indexing a file
/// keeps only the text of the error.
fn make_inserts_read_as_damage(journal: &Journal) {
    Connection::open(journal.db())
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER as_damage BEFORE INSERT ON chunk_sources BEGIN
               SELECT RAISE(ABORT, 'database disk image is malformed');
             END;",
        )
        .unwrap();
}

#[test]
fn a_failure_that_keeps_only_its_text_reaches_the_check() {
    // Through a queued save.
    let journal = Journal::indexed();
    make_inserts_read_as_damage(&journal);
    let source = journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    enqueue_path(journal.path(), &source).unwrap();
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(journal.token().is_some(), "from a save");

    // Inside a scan pass.
    let journal = Journal::indexed();
    make_inserts_read_as_damage(&journal);
    journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    scan_journal(journal.path(), false).unwrap();
    assert!(journal.token().is_some(), "from a scan pass");

    // Inside a turn. Only the first file fails, so the run commits others and
    // ends before the finish: nothing but the turn can have raised it.
    let journal = Journal::indexed();
    Connection::open(journal.db())
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER as_damage BEFORE INSERT ON chunk_sources
             WHEN NEW.path LIKE '20270101%' BEGIN
               SELECT RAISE(ABORT, 'database disk image is malformed');
             END;",
        )
        .unwrap();
    for day in 1..=9 {
        journal.save(
            &format!("2027010{day}/desktop/120000_300/talents/activity.md"),
            "kestrel",
        );
    }
    crate::todo::TEST_SCAN_LIMITS.set(Some(crate::todo::ScanLimits {
        turns_min: 4,
        turn: Duration::ZERO,
        run: Duration::ZERO,
        min_turn_files: 2,
        check_every_files: 2,
    }));
    enqueue_token(journal.path(), Token::ScanFull).unwrap();
    drain(journal.path(), DrainOptions::default()).unwrap();
    crate::todo::TEST_SCAN_LIMITS.set(None);
    assert!(
        read_entry(
            &root(journal.path()).join(
                Entry {
                    key: Token::ScanFull.key().to_owned(),
                    kind: Token::ScanFull.key()[1..].to_owned(),
                    enqueued_ms: 0,
                    attempts: 0,
                    not_before_ms: 0,
                }
                .filename()
            ),
            false
        )
        .unwrap()
        .is_some(),
        "the finish hasn't run"
    );
    assert!(journal.token().is_some(), "from a turn");
}

#[test]
fn a_second_rebuild_says_so_and_a_retried_one_does_not() {
    let journal = Journal::indexed();
    journal.scribble(&[0]);
    confirm_and_recover(&journal);
    journal.drain_until_settled();
    assert!(!rebuilt_more_than_once_this_week(journal.path()));
    TEST_CLOCK_OFFSET_MS.set(0);
    journal.checkpoint();
    journal.scribble(&[0]);
    confirm_and_recover(&journal);
    assert!(journal.token().is_none(), "a second damage still rebuilds");
    assert_eq!(read_state(journal.path()).rebuilds_ms.len(), 2);
    journal.assert_rebuilt();
    TEST_CLOCK_OFFSET_MS.set(0);
    assert!(rebuilt_more_than_once_this_week(journal.path()));
}

#[test]
fn a_reset_that_failed_once_counts_once() {
    let journal = Journal::indexed();
    TEST_RESET_FAULT.set(true);
    verdicts(&[damaged(), damaged(), damaged()]);
    report_index_damage(journal.path());
    drain(journal.path(), DrainOptions::default()).unwrap();
    advance(TEN_MINUTES + 1);
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(read_state(journal.path()).rebuilds_ms.is_empty());
    TEST_RESET_FAULT.set(false);
    advance(HOUR + 1);
    verdicts(&[damaged(), damaged(), damaged()]);
    drain(journal.path(), DrainOptions::default()).unwrap();
    advance(TEN_MINUTES + 1);
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(read_state(journal.path()).rebuilds_ms.len(), 1);
    assert!(!rebuilt_more_than_once_this_week(journal.path()));
}

#[test]
fn a_stop_during_a_check_changes_nothing() {
    // The real check, told to stop as it starts.
    let journal = Journal::indexed();
    assert_eq!(
        index_verdict(&journal.db(), &|| Err("stopped".to_owned())),
        Verdict::Stopped
    );
    // At each check in the drain: the first, the second, and the one under the lease.
    for (attempts, list) in [
        (0, vec![Verdict::Stopped]),
        (1, vec![Verdict::Stopped]),
        (1, vec![damaged(), Verdict::Stopped]),
    ] {
        let journal = Journal::indexed();
        journal.set_token(attempts, 0);
        verdicts(&list);
        let report = drain(journal.path(), DrainOptions::default()).unwrap();
        assert!(report.timed_out);
        assert_eq!(verdicts_left(), 0);
        let token = journal.token().unwrap();
        assert_eq!((token.attempts, token.not_before_ms), (attempts, 0));
        assert!(read_state(journal.path()).rebuilds_ms.is_empty());
    }
}

#[test]
fn a_rebuild_with_a_file_that_always_fails_reads_as_stated() {
    let journal = Journal::indexed();
    let bad = "20270101/desktop/120000_300/talents/activity.md";
    let path = solstone_core_format::paths::resolve_journal_path(journal.path(), bad).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
    journal.scribble(&[0]);
    confirm_and_recover(&journal);
    journal.drain_until_settled();
    assert_eq!(journal.kestrels(), NOTES as i64);
    let conn = open_index(journal.path()).unwrap();
    let receipt = read_last_scan(&conn).unwrap().unwrap();
    assert_eq!(receipt.failed, 1);
    // Like any manual reset today: the build stays unfinished until the file is
    // fixed, and the failed file is what says why.
    let state = crate::db::read_index_build_state(&conn).unwrap().unwrap();
    assert_eq!(state.state, crate::db::IndexBuildLifecycle::Building);
}

// A real kill at each step of recovery, from a child running this same binary.

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_kill_at_each_step_of_recovery_ends_with_a_complete_index() {
    if let Some(journal) = std::env::var_os("SOLSTONE_TEST_RECOVERY_CHILD") {
        drain(Path::new(&journal), DrainOptions::default()).unwrap();
        return;
    }
    for step in [
        "recovery-queued",
        "recovery-emptied",
        "recovery-prepared",
        "recovery-searchable",
    ] {
        let journal = Journal::indexed();
        journal.scribble(&[0]);
        journal.set_token(1, 0);
        let seam = journal.path().join("seam");
        fs::create_dir_all(&seam).unwrap();
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "recovery::tests::a_kill_at_each_step_of_recovery_ends_with_a_complete_index",
                    "--nocapture",
                ])
                .env("SOLSTONE_TEST_RECOVERY_CHILD", journal.path())
                .env("SOLSTONE_TEST_INDEX_WRITER_SEAM", &seam)
                .env("SOLSTONE_TEST_INDEX_WRITER_PAUSE", step)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let ready = seam.join(format!("{step}.ready"));
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !ready.exists() {
            assert!(std::time::Instant::now() < deadline, "{step} never reached");
            std::thread::sleep(Duration::from_millis(10));
        }
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        // What's left runs its course, and any further wait passes.
        for _ in 0..6 {
            advance(TEN_MINUTES + 1);
            drain(journal.path(), DrainOptions::default()).unwrap();
        }
        journal.assert_rebuilt();
        assert!(journal.token().is_none(), "{step}");
        let rebuilds = read_state(journal.path()).rebuilds_ms.len();
        assert!(rebuilds <= 1, "{step}: {rebuilds} rebuilds recorded");
    }
}

#[test]
fn a_full_disk_rewriting_the_token_holds_nothing() {
    let journal = Journal::indexed();
    report_index_damage(journal.path());
    TEST_TOKEN_WRITE_FAULT.set(true);
    for verdict in [damaged(), inconclusive()] {
        verdicts(&[verdict]);
        let report = drain(journal.path(), DrainOptions::default()).unwrap();
        assert!(!report.timed_out, "nothing recorded, so nothing is held");
        assert_eq!(journal.token().unwrap().attempts, 0);
    }
    assert_eq!(journal.kestrels(), NOTES as i64);
}

#[test]
fn a_held_or_waiting_check_is_not_work_on_the_list() {
    let journal = Journal::indexed();
    report_index_damage(journal.path());
    let status = crate::todo::inspect(journal.path()).unwrap();
    assert!(status.index_check);
    assert_eq!(status.pending, 0);
    assert!(status.keys.is_empty());
    assert_eq!(status.oldest_enqueued_ms, None);
}
