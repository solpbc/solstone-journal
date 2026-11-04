// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A whole-journal pass over a large difference takes turns with saves, and a
//! small one runs as it always has.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use super::*;
use crate::db::{db_path, open_index, reset_index};
use crate::generation::read_last_scan;
use crate::scan::scan_journal;
use rusqlite::OptionalExtension;
use solstone_core_journal_io::locking::hold_lock;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

const LIMITS: ScanLimits = ScanLimits {
    turns_min: 4,
    turn: Duration::from_secs(60),
    run: Duration::from_secs(3_600),
    min_turn_files: 2,
    check_every_files: 2,
};

struct Journal(PathBuf);
impl Journal {
    fn new() -> Self {
        TEST_SCAN_LIMITS.set(Some(LIMITS));
        TEST_AFTER_TURN.with_borrow_mut(|hook| *hook = None);
        let root = crate::test_support::reserve_temp_path("index-rebuild");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
        Self(root)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn save(&self, rel: &str, text: &str) -> PathBuf {
        let path = solstone_core_format::paths::resolve_journal_path(self.path(), rel).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
    fn save_bytes(&self, rel: &str, bytes: &[u8]) {
        let path = solstone_core_format::paths::resolve_journal_path(self.path(), rel).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
    }
    /// `count` notes, one a day, oldest first, each a day apart.
    fn notes(&self, count: usize) -> Vec<String> {
        (0..count)
            .map(|index| {
                let rel = note(index);
                self.save(&rel, &format!("kestrel note {index}"));
                rel
            })
            .collect()
    }
    fn queue_full_pass(&self) {
        enqueue_token(self.path(), Token::ScanFull).unwrap();
    }
    fn indexed(&self) -> Vec<String> {
        let conn = open_index(self.path()).unwrap();
        conn.prepare("SELECT path FROM files ORDER BY path")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
    fn rows(&self, rel: &str) -> i64 {
        open_index(self.path())
            .unwrap()
            .query_row("SELECT count(*) FROM chunks WHERE path=?", [rel], |row| {
                row.get(0)
            })
            .unwrap()
    }
    fn token(&self) -> Option<Entry> {
        read_entry(
            &root(self.path()).join(token_entry(Token::ScanFull).filename()),
            false,
        )
        .unwrap()
    }
    fn claims(&self) -> usize {
        entry_paths(&root(self.path()).join("claimed")).map_or(0, |paths| paths.len())
    }
    fn attempts(&self) -> Vec<serde_json::Value> {
        let log = self
            .path()
            .join("chronicle")
            .join(chrono::Utc::now().format("%Y%m%d").to_string())
            .join("health/indexer.jsonl");
        fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn attempt_position(&self, key: &str) -> Option<usize> {
        self.attempts()
            .iter()
            .position(|event| event["path"] == key)
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        TEST_SCAN_LIMITS.set(None);
        TEST_AFTER_TURN.with_borrow_mut(|hook| *hook = None);
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn token_entry(token: Token) -> Entry {
    Entry {
        key: token.key().to_owned(),
        kind: token.key()[1..].to_owned(),
        enqueued_ms: 0,
        attempts: 0,
        not_before_ms: 0,
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

fn after_turn(hook: impl FnMut() + 'static) {
    TEST_AFTER_TURN.with_borrow_mut(|slot| *slot = Some(Box::new(hook)));
}

/// Turns run so far, counted through the hook.
fn count_turns() -> Rc<Cell<usize>> {
    let turns = Rc::new(Cell::new(0));
    let counted = Rc::clone(&turns);
    after_turn(move || counted.set(counted.get() + 1));
    turns
}

fn drain_until_done(journal: &Journal, options: DrainOptions) -> usize {
    for round in 1..=200 {
        let report = drain(journal.path(), options).unwrap();
        assert_eq!(report.failed, 0, "round {round}");
        if journal.token().is_none() && journal.claims() == 0 {
            return round;
        }
    }
    panic!("the rebuild never finished");
}

#[test]
fn a_small_full_scan_runs_as_it_did() {
    let journal = Journal::new();
    // Under the threshold: three files, and turns_min is four.
    let rels = journal.notes(3);
    journal.queue_full_pass();
    let turns = count_turns();
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(turns.get(), 0);
    assert_eq!((report.processed, report.failed), (1, 0));
    let attempts = journal.attempts();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["path"], "@scan-full");
    assert_eq!(attempts[0]["outcome"], "indexed");
    assert!(journal.token().is_none());
    assert_eq!(journal.indexed(), rels);
    let conn = open_index(journal.path()).unwrap();
    let receipt = read_last_scan(&conn).unwrap().unwrap();
    assert!(receipt.full);
    assert_eq!(receipt.indexed, 3);
}

#[test]
fn files_decided_by_content_do_not_count_as_a_difference() {
    let journal = Journal::new();
    // The scan looks inside these every pass, so a healthy journal must not
    // read them as work for turns.
    for index in 0..10 {
        let day = note(index);
        let rel = day.replace("activity.md", "screen_text.md");
        journal.save(&rel, "# screen text\n\n## 00:12 · terminal\n- kestrel\n");
    }
    scan_journal(journal.path(), true).unwrap();
    journal.queue_full_pass();
    let turns = count_turns();
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(turns.get(), 0);
    assert_eq!((report.processed, report.failed), (1, 0));
}

#[test]
fn a_full_scan_with_too_little_budget_still_waits() {
    let journal = Journal::new();
    journal.notes(10);
    journal.queue_full_pass();
    let turns = count_turns();
    let report = drain(
        journal.path(),
        DrainOptions {
            force: false,
            budget: TOKEN_MARGIN - Duration::from_secs(1),
        },
    )
    .unwrap();
    assert_eq!(turns.get(), 0);
    assert!(!report.timed_out);
    assert_eq!(journal.token().unwrap().attempts, 0);
    assert!(journal.indexed().is_empty());
}

#[test]
fn a_plan_used_up_with_too_little_budget_leaves_the_finish_for_the_next_drain() {
    let journal = Journal::new();
    journal.notes(10);
    journal.queue_full_pass();
    // Use up the room above the margin while the turns run.
    after_turn(|| std::thread::sleep(Duration::from_millis(300)));
    let report = drain(
        journal.path(),
        DrainOptions {
            force: false,
            budget: TOKEN_MARGIN + Duration::from_millis(250),
        },
    )
    .unwrap();
    assert!(!report.timed_out);
    assert_eq!(report.processed, 0);
    assert_eq!(journal.token().unwrap().attempts, 0);
    assert_eq!(journal.claims(), 0);
    assert_eq!(drain_until_done(&journal, DrainOptions::default()), 1);
}

fn count_files(path: &Path) -> usize {
    crate::db::open_index_reader(path).map_or(0, |conn| {
        conn.query_row("SELECT count(*) FROM files", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0) as usize
    })
}

#[test]
fn a_save_during_a_rebuild_is_served_within_one_turn() {
    let journal = Journal::new();
    journal.notes(40);
    journal.queue_full_pass();
    let fresh = "20270101/desktop/120000_300/talents/activity.md";
    let source = journal.save(fresh, "a fresh kestrel note");
    let saved_at = Cell::new(None);
    let served_at = Cell::new(None);
    // Turns are long here, so only a waiting save can end one. The save arrives
    // part way through the first turn, on a commit.
    let report = drain_with_checkpoint(journal.path(), DrainOptions::default(), &|| {
        let files = count_files(journal.path());
        if saved_at.get().is_none() && files >= 3 {
            enqueue_path(journal.path(), &source).map_err(|error| error.to_string())?;
            saved_at.set(Some(files));
        }
        if served_at.get().is_none() && journal.attempt_position(fresh).is_some() {
            served_at.set(Some(files));
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(report.failed, 0);
    let (before, after) = (saved_at.get().unwrap(), served_at.get().unwrap());
    // At most what a turn does before it asks, and the save itself.
    assert!(
        after - before <= LIMITS.check_every_files + 1,
        "saved at {before} files, served at {after}"
    );
    assert!(after < 40, "served at {after}");
    assert_eq!(journal.rows(fresh), 1);
    assert_eq!(journal.indexed().len(), 41);
    assert!(journal.token().is_none());
}

#[test]
fn a_file_saved_again_is_served_again_during_a_rebuild() {
    let journal = Journal::new();
    journal.notes(40);
    journal.queue_full_pass();
    let fresh = "20270101/desktop/120000_300/talents/activity.md";
    let source = journal.save(fresh, "kestrel");
    let saved = Cell::new(0);
    drain_with_checkpoint(journal.path(), DrainOptions::default(), &|| {
        let served = journal
            .attempts()
            .iter()
            .filter(|event| event["path"] == fresh)
            .count();
        let files = count_files(journal.path());
        // First once the run is under way, then again once that was served.
        if (saved.get() == 0 && files >= 3) || (saved.get() == 1 && served == 1) {
            saved.set(saved.get() + 1);
            enqueue_path(journal.path(), &source).map_err(|error| error.to_string())?;
        }
        Ok(())
    })
    .unwrap();
    let served = journal
        .attempts()
        .iter()
        .filter(|event| event["path"] == fresh)
        .count();
    assert_eq!(served, 2);
}

#[test]
fn a_failing_path_is_tried_once_per_drain_during_a_rebuild() {
    let journal = Journal::new();
    journal.notes(40);
    journal.queue_full_pass();
    // Not UTF-8, so indexing it fails every time.
    let bad = "20270101/desktop/120000_300/talents/activity.md";
    journal.save_bytes(bad, &[0xff, 0xfe, 0xfd]);
    let source = solstone_core_format::paths::resolve_journal_path(journal.path(), bad).unwrap();
    let queued = Cell::new(false);
    // Forced, so only being served spaces out its retries.
    let report = drain_with_checkpoint(
        journal.path(),
        DrainOptions {
            force: true,
            ..DrainOptions::default()
        },
        &|| {
            if !queued.get() && count_files(journal.path()) >= 3 {
                queued.set(true);
                enqueue_path(journal.path(), &source).map_err(|error| error.to_string())?;
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(report.failed, 1);
    let tried = journal
        .attempts()
        .iter()
        .filter(|event| event["path"] == bad)
        .count();
    assert_eq!(tried, 1);
}

#[test]
fn a_failed_path_coming_due_does_not_shrink_the_turns() {
    let journal = Journal::new();
    journal.notes(60);
    journal.queue_full_pass();
    let bad = "20270101/desktop/120000_300/talents/activity.md";
    journal.save_bytes(bad, &[0xff, 0xfe, 0xfd]);
    let source = solstone_core_format::paths::resolve_journal_path(journal.path(), bad).unwrap();
    let queued = Cell::new(false);
    let turns = count_turns();
    // Forced, so the failed path is due again at once. Turns are long, so only
    // a path that is due can end one: if it did, there would be dozens.
    let report = drain_with_checkpoint(
        journal.path(),
        DrainOptions {
            force: true,
            ..DrainOptions::default()
        },
        &|| {
            if !queued.get() && count_files(journal.path()) >= 3 {
                queued.set(true);
                enqueue_path(journal.path(), &source).map_err(|error| error.to_string())?;
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(report.failed, 1);
    assert!(turns.get() <= 3, "{} turns", turns.get());
    // A token is never a save.
    assert!(
        !solstone_core_indexer_todo::any_due_path(journal.path(), u64::MAX, &|key| key == bad)
            .unwrap()
    );
}

#[test]
fn a_small_full_pass_is_not_put_off_by_a_file_saved_again_during_its_walk() {
    let journal = Journal::new();
    // Under the threshold, with one path served ahead of it.
    let rels = journal.notes(3);
    journal.queue_full_pass();
    let source =
        solstone_core_format::paths::resolve_journal_path(journal.path(), &rels[0]).unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    let saved = Cell::new(false);
    let report = drain_with_checkpoint(journal.path(), DrainOptions::default(), &|| {
        // Once that path is served, the walk runs past the drain's slice and
        // the file is saved again.
        if !saved.get() && journal.attempt_position(&rels[0]).is_some() {
            saved.set(true);
            std::thread::sleep(DRAIN_SLICE + Duration::from_millis(50));
            enqueue_path(journal.path(), &source).map_err(|error| error.to_string())?;
        }
        Ok(())
    })
    .unwrap();
    assert!(saved.get());
    assert_eq!(report.failed, 0);
    assert!(journal.token().is_none(), "the full pass was put off");
}

#[test]
fn a_rebuild_releases_the_writer_between_turns_newest_day_first() {
    let journal = Journal::new();
    let rels = journal.notes(12);
    journal.queue_full_pass();
    TEST_SCAN_LIMITS.set(Some(ScanLimits {
        turn: Duration::ZERO,
        ..LIMITS
    }));
    let path = journal.path().to_path_buf();
    let seen: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let seen = Rc::clone(&seen);
        after_turn(move || {
            // The lease is free: a zero-timeout hold succeeds.
            let lease = solstone_core_journal_io::locking::hold_lock(
                db_path(&path),
                LockOptions {
                    timeout: Duration::ZERO,
                    ..LockOptions::default()
                },
            );
            assert!(lease.is_ok(), "the writer was not released between turns");
            drop(lease);
            let conn = crate::db::open_index_reader(&path).unwrap();
            let files = conn
                .prepare("SELECT path FROM files ORDER BY path")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            seen.borrow_mut().push(files);
        });
    }
    drain(journal.path(), DrainOptions::default()).unwrap();
    let seen = seen.borrow();
    assert!(seen.len() >= 2, "it took {} turn(s)", seen.len());
    // The first turn holds only the newest days.
    let first = &seen[0];
    assert_eq!(first.len(), LIMITS.min_turn_files);
    assert_eq!(first.as_slice(), &rels[rels.len() - first.len()..]);
    assert_eq!(journal.indexed(), rels);
}

#[test]
fn a_rebuild_stopped_between_runs_finishes_with_every_file_current_and_no_attempt_spent() {
    let journal = Journal::new();
    let rels = journal.notes(12);
    journal.queue_full_pass();
    TEST_SCAN_LIMITS.set(Some(ScanLimits {
        turn: Duration::ZERO,
        run: Duration::ZERO,
        ..LIMITS
    }));
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert!(!report.timed_out);
    assert_eq!(report.remaining.keys, ["@scan-full"]);
    assert_eq!(journal.token().unwrap().attempts, 0);
    assert_eq!(journal.claims(), 0);
    assert_eq!(journal.indexed().len(), LIMITS.min_turn_files);
    // Every drain makes progress, even when its run is already over.
    let rounds = drain_until_done(&journal, DrainOptions::default());
    assert!(rounds > 1, "{rounds}");
    assert_eq!(journal.indexed(), rels);
}

#[test]
fn a_rebuild_stopped_by_its_budget_spends_no_attempt() {
    let journal = Journal::new();
    let rels = journal.notes(12);
    journal.queue_full_pass();
    let commits = Cell::new(0);
    let report = drain_with_checkpoint(journal.path(), DrainOptions::default(), &|| {
        if count_files(journal.path()) > 0 {
            commits.set(commits.get() + 1);
        }
        if commits.get() > 3 {
            Err("index work wait expired".to_owned())
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert!(report.timed_out);
    assert_eq!(report.failed, 0);
    assert_eq!(journal.token().unwrap().attempts, 0);
    assert_eq!(journal.claims(), 0);
    drain_until_done(&journal, DrainOptions::default());
    assert_eq!(journal.indexed(), rels);
}

#[test]
fn a_run_that_keeps_dying_before_committing_reaches_todays_claim() {
    let journal = Journal::new();
    let rels = journal.notes(12);
    journal.queue_full_pass();
    let die = || {
        let dying = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drain_with_checkpoint(journal.path(), DrainOptions::default(), &|| {
                if journal.token().is_some_and(|token| token.attempts > 0) {
                    panic!("the process died");
                }
                Ok(())
            })
        }));
        assert!(dying.is_err());
    };
    let attempts_before = journal.token().unwrap().attempts;
    assert_eq!(attempts_before, 0);
    die();
    assert_eq!(journal.token().unwrap().attempts, 1);
    // The attempt held for the crash stays, and a second run adds to it.
    TEST_AFTER_TURN.with_borrow_mut(|hook| *hook = None);
    let dying_again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drain_with_checkpoint(journal.path(), DrainOptions::default(), &|| {
            if journal.token().is_some_and(|token| token.attempts > 1) {
                panic!("the process died");
            }
            Ok(())
        })
    }));
    assert!(dying_again.is_err());
    assert_eq!(journal.token().unwrap().attempts, 2);
    // Now no plan and no turns: today's claim and pass, in one go.
    let turns = count_turns();
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(turns.get(), 0);
    assert_eq!((report.processed, report.failed), (1, 0));
    assert_eq!(journal.indexed(), rels);
}

#[test]
fn other_tokens_wait_for_the_finish_and_paths_do_not() {
    let journal = Journal::new();
    journal.notes(20);
    journal.queue_full_pass();
    // Many short turns, so the other tokens have every chance to jump in.
    TEST_SCAN_LIMITS.set(Some(ScanLimits {
        turn: Duration::ZERO,
        ..LIMITS
    }));
    enqueue_token(journal.path(), Token::Reclassify).unwrap();
    enqueue_token(journal.path(), Token::EntitySearch).unwrap();
    let fresh = "20270101/desktop/120000_300/talents/activity.md";
    let path = journal.path().to_path_buf();
    let queued = Rc::new(Cell::new(false));
    {
        let queued = Rc::clone(&queued);
        after_turn(move || {
            if !queued.get() {
                queued.set(true);
                let source =
                    solstone_core_format::paths::resolve_journal_path(&path, fresh).unwrap();
                fs::create_dir_all(source.parent().unwrap()).unwrap();
                fs::write(&source, "kestrel").unwrap();
                enqueue_path(&path, &source).unwrap();
            }
        });
    }
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(report.failed, 0);
    let order = journal.attempts();
    let at = |key: &str| {
        order
            .iter()
            .position(|event| event["path"] == key)
            .unwrap_or_else(|| panic!("{key} never ran: {order:?}"))
    };
    assert!(at(fresh) < at("@scan-full"));
    assert!(at("@scan-full") < at("@reclassify"));
    assert!(at("@reclassify") < at("@entity-search"));
}

#[test]
fn a_file_recreated_during_a_rebuild_keeps_its_rows_and_a_deleted_one_goes() {
    let journal = Journal::new();
    let rels = journal.notes(12);
    // A complete index whose files all changed: a mass move's shape.
    scan_journal(journal.path(), true).unwrap();
    for rel in &rels {
        let path = solstone_core_format::paths::resolve_journal_path(journal.path(), rel).unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(120);
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(later).unwrap();
    }
    journal.queue_full_pass();
    let (gone, recreated) = (rels[0].clone(), rels[1].clone());
    let path = journal.path().to_path_buf();
    let done = Rc::new(Cell::new(false));
    {
        let done = Rc::clone(&done);
        let (gone, recreated) = (gone.clone(), recreated.clone());
        after_turn(move || {
            if !done.get() {
                done.set(true);
                let gone = solstone_core_format::paths::resolve_journal_path(&path, &gone).unwrap();
                fs::remove_file(&gone).unwrap();
                let recreated =
                    solstone_core_format::paths::resolve_journal_path(&path, &recreated).unwrap();
                fs::remove_file(&recreated).unwrap();
                fs::write(&recreated, "kestrel recreated").unwrap();
                enqueue_path(&path, &recreated).unwrap();
            }
        });
    }
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(report.failed, 0);
    assert!(journal.token().is_none());
    assert_eq!(journal.rows(&recreated), 1);
    assert_eq!(journal.rows(&gone), 0);
    assert!(!journal.indexed().contains(&gone));
    assert_eq!(journal.indexed().len(), rels.len() - 1);
}

#[test]
fn a_rebuild_with_a_file_that_always_fails_converges() {
    let journal = Journal::new();
    let rels = journal.notes(12);
    let bad = "20270101/desktop/120000_300/talents/activity.md";
    journal.save_bytes(bad, &[0xff, 0xfe, 0xfd]);
    journal.queue_full_pass();
    TEST_SCAN_LIMITS.set(Some(ScanLimits {
        turn: Duration::ZERO,
        run: Duration::ZERO,
        ..LIMITS
    }));
    drain_until_done(&journal, DrainOptions::default());
    assert_eq!(journal.indexed(), rels);
    let conn = open_index(journal.path()).unwrap();
    let receipt = read_last_scan(&conn).unwrap().unwrap();
    assert_eq!(receipt.failed, 1);
    assert_eq!(receipt.failures[0].path.as_deref(), Some(bad));
}

#[test]
fn a_rebuild_whose_every_insert_fails_lands_in_todays_accounting() {
    let journal = Journal::new();
    for index in 0..12 {
        journal.save_bytes(&note(index), &[0xff, 0xfe, 0xfd]);
    }
    journal.queue_full_pass();
    // A run with no commits: at its limit it stops taking turns.
    TEST_SCAN_LIMITS.set(Some(ScanLimits {
        turn: Duration::ZERO,
        run: Duration::ZERO,
        ..LIMITS
    }));
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!((report.processed, report.failed), (1, 0));
    assert!(journal.token().is_none());
    let conn = open_index(journal.path()).unwrap();
    let receipt = read_last_scan(&conn).unwrap().unwrap();
    assert_eq!(receipt.failed, 12);
    assert_eq!(receipt.indexed, 0);
}

#[test]
fn a_rebuild_whose_writes_fail_lands_in_todays_accounting() {
    let journal = Journal::new();
    journal.notes(12);
    open_index(journal.path())
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER no_files BEFORE INSERT ON files BEGIN SELECT RAISE(ABORT, 'no'); END;",
        )
        .unwrap();
    journal.queue_full_pass();
    let forced = DrainOptions {
        force: true,
        ..DrainOptions::default()
    };
    let turns = count_turns();
    // The turn hits the error, and the token goes to today's claim, which counts
    // a failure instead of looping quietly.
    let report = drain(journal.path(), forced).unwrap();
    assert_eq!(turns.get(), 1);
    assert_eq!((report.processed, report.failed), (0, 1));
    assert_eq!(journal.token().unwrap().attempts, 1);
    let receipt = read_last_scan(&open_index(journal.path()).unwrap())
        .unwrap()
        .unwrap();
    assert!(receipt.error.is_some());
    // Twice more and it reads failing, with hourly retries, as any token does.
    assert_eq!(drain(journal.path(), forced).unwrap().failed, 1);
    assert_eq!(drain(journal.path(), forced).unwrap().failed, 1);
    let status = inspect(journal.path()).unwrap();
    assert_eq!(status.failed_tokens, 1);
    // And the daily check isn't left off by it: it runs, and meets the same error.
    assert!(scan_journal(journal.path(), false).is_err());
}

#[test]
fn a_rebuild_leaves_connections_to_their_slices() {
    let journal = Journal::new();
    journal.save(
        "entities/alice/entity.json",
        r#"{"name":"Alice Edge","type":"Person","aka":["Ally"]}"#,
    );
    journal.save(
        "entities/bob/entity.json",
        r#"{"name":"Bob Edge","type":"Person"}"#,
    );
    let mut edge_files = Vec::new();
    for index in 0..12 {
        let rel = format!("facets/work/entities/{}.jsonl", 20260101 + index);
        journal.save(
            &rel,
            "{\"name\":\"Ally\",\"segments\":[\"s1\"]}\n{\"name\":\"Bob Edge\",\"segments\":[\"s1\"]}\n",
        );
        edge_files.push(rel);
    }
    for rel in [
        "facets/work/entities/alice/entity.json",
        "facets/work/entities/bob/entity.json",
    ] {
        journal.save(rel, "{}");
    }
    // The retired-plan recheck reads these two kinds. They must stay handed off.
    let event = "facets/work/events/20260101.jsonl";
    let activity = "facets/work/activities/20260101.jsonl";
    journal.save(event, "{\"title\":\"a meeting\",\"occurred\":true}\n");
    journal.save(activity, "{\"anticipated\":true,\"id\":\"a\"}\n");
    journal.queue_full_pass();
    drain_until_done_keeping_edges(&journal);
    for rel in [event, activity] {
        let mtime: Option<i64> = open_index(journal.path())
            .unwrap()
            .query_row("SELECT mtime FROM edge_files WHERE path=?", [rel], |row| {
                row.get(0)
            })
            .optional()
            .unwrap();
        assert_eq!(mtime, Some(crate::edge_refresh::STALE_EDGE_MTIME), "{rel}");
    }
    let stale = |journal: &Journal| -> i64 {
        open_index(journal.path())
            .unwrap()
            .query_row(
                "SELECT count(*) FROM edge_files WHERE mtime=-1 AND path NOT LIKE 'edges:%'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    // The finish extracted none of them under its lease.
    assert!(
        stale(&journal) > LIMITS.turns_min as i64,
        "{}",
        stale(&journal)
    );
    assert_eq!(
        open_index(journal.path())
            .unwrap()
            .query_row("SELECT count(*) FROM edges", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    // The slices then bring them all current.
    for _ in 0..20 {
        drain(journal.path(), DrainOptions::default()).unwrap();
        if stale(&journal) == 0 {
            break;
        }
    }
    assert_eq!(stale(&journal), 0);
    let rows: i64 = open_index(journal.path())
        .unwrap()
        .query_row("SELECT count(*) FROM edges", [], |row| row.get(0))
        .unwrap();
    assert!(rows > 0);
}

/// Run a drain whose connection slices can't run: the slice comes after the
/// finish in the same drain, so stop there by giving it no room.
fn drain_until_done_keeping_edges(journal: &Journal) {
    let finished = Cell::new(false);
    let report = drain_with_checkpoint(journal.path(), DrainOptions::default(), &|| {
        if finished.get() {
            return Err("index work wait expired".to_owned());
        }
        if !journal.attempts().is_empty() {
            finished.set(true);
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(report.failed, 0);
}

#[test]
fn a_cli_full_rescan_still_extracts_connections_itself() {
    let journal = Journal::new();
    journal.save(
        "entities/alice/entity.json",
        r#"{"name":"Alice Edge","type":"Person","aka":["Ally"]}"#,
    );
    journal.save("facets/work/entities/alice/entity.json", "{}");
    for index in 0..12 {
        journal.save(
            &format!("facets/work/entities/{}.jsonl", 20260101 + index),
            "{\"name\":\"Ally\",\"segments\":[\"s1\"]}\n",
        );
    }
    scan_journal(journal.path(), true).unwrap();
    let stale: i64 = open_index(journal.path())
        .unwrap()
        .query_row(
            "SELECT count(*) FROM edge_files WHERE mtime=-1 AND path NOT LIKE 'edges:%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0);
    assert!(listed_keys(&journal).is_empty());
}

fn listed_keys(journal: &Journal) -> Vec<String> {
    inspect(journal.path()).unwrap().keys
}

#[test]
fn the_daily_check_stands_down_only_for_a_large_difference_during_a_full_pass() {
    let journal = Journal::new();
    let rels = journal.notes(12);
    // No token: today's behaviour.
    let report = scan_journal(journal.path(), false).unwrap();
    assert!(!report.deferred_to_full);
    assert_eq!(report.indexed, rels.len());

    // A token and a large difference: it leaves the work and records nothing.
    reset_index(journal.path()).unwrap();
    journal.queue_full_pass();
    let report = scan_journal(journal.path(), false).unwrap();
    assert!(report.deferred_to_full);
    assert_eq!(report.indexed, 0);
    assert!(journal.indexed().is_empty());
    let receipt = read_last_scan(&open_index(journal.path()).unwrap()).unwrap();
    assert!(receipt.is_none());

    // A token that keeps failing never turns the daily check off.
    let token = root(journal.path()).join(token_entry(Token::ScanFull).filename());
    let mut entry = read_entry(&token, false).unwrap().unwrap();
    entry.attempts = 3;
    rewrite(&token, &entry).unwrap();
    let report = scan_journal(journal.path(), false).unwrap();
    assert!(!report.deferred_to_full);
    assert_eq!(report.indexed, rels.len());

    // A token and a small difference: today's behaviour.
    entry.attempts = 0;
    rewrite(&token, &entry).unwrap();
    journal.save("20270101/desktop/120000_300/talents/activity.md", "kestrel");
    let report = scan_journal(journal.path(), false).unwrap();
    assert!(!report.deferred_to_full);
    assert_eq!(report.indexed, 1);
}

#[test]
fn a_forced_drain_succeeds_during_a_rebuild() {
    let journal = Journal::new();
    journal.notes(60);
    journal.queue_full_pass();
    let fresh = "20270101/desktop/120000_300/talents/activity.md";
    let source = journal.save(fresh, "kestrel");
    let limits = ScanLimits {
        turn: Duration::ZERO,
        run: Duration::from_millis(200),
        ..LIMITS
    };
    let path = journal.path().to_path_buf();
    let (mid_run, is_mid_run) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        TEST_SCAN_LIMITS.set(Some(limits));
        after_turn(move || {
            let _ = mid_run.send(());
        });
        let report = drain(&path, DrainOptions::default()).unwrap();
        TEST_AFTER_TURN.with_borrow_mut(|hook| *hook = None);
        TEST_SCAN_LIMITS.set(None);
        report
    });
    // The other drain has taken a turn and holds the drain lock.
    is_mid_run.recv().unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    TEST_SCAN_LIMITS.set(Some(limits));
    let report = drain(
        journal.path(),
        DrainOptions {
            force: true,
            budget: Duration::from_secs(600),
        },
    )
    .unwrap();
    assert!(!report.timed_out);
    assert_eq!(report.failed, 0);
    let held = holder.join().unwrap();
    assert!(!held.timed_out);
    assert_eq!(journal.rows(fresh), 1);
    assert!(hold_lock(db_path(journal.path()), LockOptions::default()).is_ok());
}

/// Noise over whole pages of the index file.
fn scribble(path: &Path, pages: &[u64]) {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
    let noise: Vec<u8> = (0..4096u32)
        .map(|byte| (byte.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    for page in pages {
        file.seek(SeekFrom::Start(page * 4096)).unwrap();
        file.write_all(&noise).unwrap();
    }
}

#[test]
fn reset_with_a_live_wal_and_a_reader_in_another_process() {
    use rusqlite::{Connection, OpenFlags};
    let kestrels = "SELECT count(*) FROM chunks WHERE chunks MATCH 'kestrel'";
    // The other process, run from this same test binary.
    if let Some(path) = std::env::var_os("SOLSTONE_TEST_RESET_READER") {
        let path = PathBuf::from(path);
        let reader = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut statement = reader
            .prepare("SELECT rowid FROM chunks WHERE chunks MATCH 'kestrel'")
            .unwrap();
        let mut rows = statement.query([]).unwrap();
        assert!(rows.next().unwrap().is_some());
        fs::write(path.with_extension("reader-ready"), b"ready").unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while !path.with_extension("reader-release").exists() {
            assert!(Instant::now() < deadline, "never released");
            std::thread::sleep(Duration::from_millis(20));
        }
        // The old read may finish or fail. It must not hang or crash.
        while let Ok(Some(_)) = rows.next() {}
        drop(rows);
        drop(statement);
        drop(reader);
        // A fresh read sees the new index.
        let fresh = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let count: i64 = fresh.query_row(kestrels, [], |row| row.get(0)).unwrap();
        assert_eq!(count, 300);
        return;
    }
    let journal = Journal::new();
    journal.notes(300);
    scan_journal(journal.path(), true).unwrap();
    let path = db_path(journal.path());
    Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    // Frames left in the WAL by a writer still attached.
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch(
            "PRAGMA wal_autocheckpoint=0; INSERT INTO files(path, mtime) VALUES ('wal-only', 1);",
        )
        .unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "todo::rebuild_tests::reset_with_a_live_wal_and_a_reader_in_another_process",
            "--nocapture",
        ])
        .env("SOLSTONE_TEST_RESET_READER", &path)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !path.with_extension("reader-ready").exists() {
        assert!(Instant::now() < deadline, "reader never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let pages = fs::metadata(&path).unwrap().len() / 4096;
    scribble(&path, &[pages * 3 / 4, pages / 2, pages / 3]);
    crate::db::empty_damaged_index(&path).expect("empty a damaged index with a reader attached");
    drop(writer);
    reset_index(journal.path()).unwrap();
    scan_journal(journal.path(), true).unwrap();
    fs::write(path.with_extension("reader-release"), b"go").unwrap();
    assert!(child.wait().unwrap().success());
}
