// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Operational log retention over real journal trees.
//!
//! ⚠ In `tests/` because `tests/architecture.rs` forbids any `src/` module from naming
//! a removal primitive and a filesystem bed has to tear itself down.

#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "bed setup and teardown; the crate-wide bans exist to constrain the \
              production verbs"
)]

use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;
use solstone_core_retention::door::remove_logs;
use solstone_core_retention::logs::{CLASSES, EntryKind, Kept, LogPolicy, LogTarget, plan};

struct Bed {
    root: PathBuf,
}

impl Bed {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "retention-logs-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("a bed");
        Self { root }
    }

    fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().expect("a parent")).expect("dirs");
        fs::write(&path, bytes).expect("write");
        path
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let path = self.root.join(rel);
        fs::create_dir_all(&path).expect("dirs");
        path
    }

    fn plan(&self, days: u32, today: &str) -> solstone_core_retention::logs::LogPlan {
        plan(
            &self.root,
            &LogPolicy {
                days,
                enabled: true,
            },
            date(today),
        )
    }
}

fn date(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("a date")
}

fn teardown(bed: &Bed) {
    fs::remove_dir_all(&bed.root).expect("teardown");
}

fn rels(targets: &[LogTarget]) -> Vec<&str> {
    let mut found: Vec<&str> = targets.iter().map(LogTarget::rel).collect();
    found.sort_unstable();
    found
}

/// 🔴 Off by default, like the media policy.
#[test]
fn the_default_policy_prunes_nothing() {
    let bed = Bed::new("default-off");
    bed.file("tokens/20200101.jsonl", b"old");
    let built = plan(&bed.root, &LogPolicy::default(), date("2026-08-05"));
    assert!(built.prunable.is_empty());
    assert_eq!(built.examined(), 0, "a disabled pass does not even walk");
    teardown(&bed);
}

/// ⛔ An unset window keeps; it is not a zero-day window that prunes everything.
#[test]
fn a_zero_day_window_prunes_nothing() {
    let bed = Bed::new("zero-days");
    bed.file("tokens/20200101.jsonl", b"old");
    let built = plan(
        &bed.root,
        &LogPolicy {
            days: 0,
            enabled: true,
        },
        date("2026-08-05"),
    );
    assert!(
        built.prunable.is_empty(),
        "days: 0 with enabled: true must not delete the journal's logs"
    );
    teardown(&bed);
}

/// The plain case, across every stem-dated class.
#[test]
fn stem_dated_classes_prune_past_the_window_and_keep_inside_it() {
    let bed = Bed::new("stem");
    for base in [
        "tokens",
        "health/local-inference",
        "awareness",
        "config/actions",
        "health/pruning-runs",
    ] {
        bed.file(&format!("{base}/20260101.jsonl"), b"old");
        bed.file(&format!("{base}/20260804.jsonl"), b"fresh");
    }

    let built = bed.plan(7, "2026-08-05");
    assert_eq!(
        rels(&built.prunable),
        vec![
            "awareness/20260101.jsonl",
            "config/actions/20260101.jsonl",
            "health/local-inference/20260101.jsonl",
            "health/pruning-runs/20260101.jsonl",
            "tokens/20260101.jsonl",
        ]
    );
    assert!(
        built
            .retained
            .iter()
            .filter(|kept| matches!(kept.reason, Kept::TooYoung(_)))
            .count()
            >= 5
    );
    teardown(&bed);
}

/// 🔴 The class the reference does not have: the pruner now prunes its own runs.
#[test]
fn the_pruning_run_records_are_themselves_prunable() {
    let bed = Bed::new("pruning-runs");
    bed.file("health/pruning-runs/20260101.jsonl", b"a run record");

    let built = bed.plan(7, "2026-08-05");
    let mine = built.by_class("pruning_runs");
    assert_eq!(mine.len(), 1, "{built:?}");
    assert_eq!(mine[0].rel(), "health/pruning-runs/20260101.jsonl");

    // And it is genuinely NOT covered by the class that looks like it should be.
    assert!(
        built
            .by_class("chronicle_health_logs")
            .iter()
            .all(|target| !target.rel().starts_with("health/")),
        "chronicle_health_logs walks chronicle/<day>/health, not the journal root's"
    );
    teardown(&bed);
}

/// The expanded-directory classes, each dated its own way.
#[test]
fn expanded_classes_walk_exactly_one_level() {
    let bed = Bed::new("expand");
    // Dated by the expanded day component, not the filename.
    bed.file("chronicle/20260101/health/observer.log", b"old");
    bed.file("chronicle/20260804/health/observer.log", b"fresh");
    // Dated by the stem, one level under facets/.
    bed.file("facets/work/logs/20260101.jsonl", b"old");
    bed.file("facets/work/logs/20260804.jsonl", b"fresh");
    // ⛔ Two levels deep: must not be reached.
    bed.file("facets/work/logs/nested/20260101.jsonl", b"deep");

    let built = bed.plan(7, "2026-08-05");
    assert_eq!(
        rels(&built.prunable),
        vec![
            "chronicle/20260101/health/observer.log",
            "facets/work/logs/20260101.jsonl",
        ]
    );
    teardown(&bed);
}

/// Epoch-millisecond stems, and the exemption that protects a live run.
#[test]
fn a_live_talent_run_log_is_exempt_however_old() {
    let bed = Bed::new("talents");
    // 2026-01-01T00:00:00Z in milliseconds.
    bed.file("talents/scribe/1767225600000.jsonl", b"old run");
    bed.file("talents/scribe/1767225600000_active.jsonl", b"live run");
    // The day-index class sits one level up, dated YYYYMMDD.
    bed.file("talents/20260101.jsonl", b"{\"ts\":1767225600000}\n");

    let built = bed.plan(7, "2026-08-05");
    assert_eq!(
        rels(&built.prunable),
        vec![
            "talents/20260101.jsonl",
            "talents/scribe/1767225600000.jsonl"
        ]
    );
    assert!(
        built
            .retained
            .iter()
            .any(|kept| kept.rel.ends_with("1767225600000_active.jsonl")
                && kept.reason == Kept::Exempt),
        "{built:?}"
    );
    teardown(&bed);
}

/// The two talent classes are different levels of the same tree.
#[test]
fn the_talent_classes_do_not_take_each_others_entries() {
    let bed = Bed::new("talent-levels");
    bed.file("talents/20260101.jsonl", b"{\"ts\":1767225600000}\n");
    bed.file("talents/scribe/1767225600000.jsonl", b"run");

    let built = bed.plan(7, "2026-08-05");
    assert_eq!(
        built.by_class("talent_day_index")[0].rel(),
        "talents/20260101.jsonl"
    );
    assert_eq!(
        built.by_class("talent_run_logs")[0].rel(),
        "talents/scribe/1767225600000.jsonl"
    );
    teardown(&bed);
}

/// An old filename is only a first filter: every index row must also be old.
#[test]
fn talent_day_indexes_require_wholly_old_valid_rows() {
    let bed = Bed::new("talent-day-index-content");
    bed.file(
        "talents/20260101.jsonl",
        b"{\"ts\":1767225600000}\n{\"ts\":1767312000000}\n",
    );
    bed.file("talents/20260102.jsonl", b"{\"ts\":1786492800000}\n");
    bed.file("talents/20260103.jsonl", b"{not json}\n");

    let built = bed.plan(7, "2026-08-05");
    assert_eq!(
        built
            .by_class("talent_day_index")
            .iter()
            .map(|target| target.rel())
            .collect::<Vec<&str>>(),
        vec!["talents/20260101.jsonl"],
        "only the wholly old index is prunable: {built:?}"
    );
    assert!(built.retained.iter().any(|kept| {
        kept.rel == "talents/20260102.jsonl" && kept.reason == Kept::ContentNotFullyOld
    }));
    assert!(built.retained.iter().any(|kept| {
        kept.rel == "talents/20260103.jsonl" && matches!(kept.reason, Kept::ContentMalformed { .. })
    }));
    teardown(&bed);
}

/// The mtime-dated class removes directories, not files.
#[test]
fn the_cache_class_removes_directories_by_mtime() {
    let bed = Bed::new("cache");
    let old = bed.dir(".cache/cogitate-history/session-old");
    bed.file(
        ".cache/cogitate-history/session-old/events/event-00000.json",
        b"cache payload",
    );
    bed.dir(".cache/cogitate-history/session-new");
    bed.file(
        ".cache/cogitate-history/stray.txt",
        b"a file, not a session",
    );
    // Backdate the old session.
    let long_ago =
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800); // 2020-01-01
    filetime_set(&old, long_ago);

    let built = bed.plan(7, "2026-08-05");
    let mine = built.by_class("cogitate_history_cache");
    assert_eq!(mine.len(), 1, "{built:?}");
    assert_eq!(mine[0].rel(), ".cache/cogitate-history/session-old");
    assert_eq!(mine[0].kind(), EntryKind::Directory);
    assert!(
        mine[0].bytes() > 0,
        "cache content contributes reclaimed bytes"
    );
    assert!(
        built
            .retained
            .iter()
            .any(|kept| kept.rel.ends_with("stray.txt") && kept.reason == Kept::NotAMatch),
        "a file in a directory class is not a match: {built:?}"
    );

    let outcome = remove_logs(&bed.root, &[mine[0].clone()]);
    assert!(outcome.targets[0].not_removed.is_empty(), "{outcome:?}");
    assert!(!old.exists(), "the session directory is gone");
    assert!(
        bed.root
            .join(".cache/cogitate-history/session-new")
            .exists(),
        "the recent one survives"
    );
    teardown(&bed);
}

/// A file-class symlink is an entry to unlink, while an escaping parent is refused.
#[test]
fn file_classes_unlink_leaf_symlinks_but_refuse_escaping_parents() {
    use std::os::unix::fs::symlink;

    let bed = Bed::new("file-symlink");
    let external_file = std::env::temp_dir().join(format!(
        "retention-file-target-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock")
            .as_nanos()
    ));
    fs::write(&external_file, b"outside the journal").expect("external file");
    let link = bed.root.join("tokens/20260101.jsonl");
    fs::create_dir_all(link.parent().expect("tokens root")).expect("tokens root");
    symlink(&external_file, &link).expect("token link");

    let built = bed.plan(7, "2026-08-05");
    let target = built.by_class("tokens");
    assert_eq!(target.len(), 1, "the leaf link is a file target: {built:?}");
    let outcome = remove_logs(&bed.root, &[target[0].clone()]);
    assert!(outcome.targets[0].not_removed.is_empty(), "{outcome:?}");
    assert!(link.symlink_metadata().is_err(), "the link was unlinked");
    assert!(external_file.is_file(), "the link target survives");

    fs::remove_dir_all(bed.root.join("tokens")).expect("remove token directory");
    let external_dir = std::env::temp_dir().join(format!(
        "retention-file-parent-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock")
            .as_nanos()
    ));
    fs::create_dir_all(&external_dir).expect("external directory");
    let external_child = external_dir.join("20260101.jsonl");
    fs::write(&external_child, b"outside the journal").expect("external child");
    symlink(&external_dir, bed.root.join("tokens")).expect("escaping token directory");

    let built = bed.plan(7, "2026-08-05");
    let target = built.by_class("tokens");
    assert_eq!(
        target.len(),
        1,
        "the root still plans a dated file: {built:?}"
    );
    let outcome = remove_logs(&bed.root, &[target[0].clone()]);
    assert!(
        outcome.targets[0]
            .not_removed
            .iter()
            .any(|entry| entry.entry == "tokens/20260101.jsonl"),
        "the escaping parent is refused: {outcome:?}"
    );
    assert!(external_child.is_file(), "the external file survives");

    fs::remove_file(bed.root.join("tokens")).expect("remove token link");
    fs::remove_file(&external_file).expect("external file teardown");
    fs::remove_dir_all(&external_dir).expect("external directory teardown");
    teardown(&bed);
}

/// A contained directory removal refuses a cache symlink rather than following it.
#[test]
fn the_cache_class_does_not_follow_a_symlink() {
    use std::os::unix::fs::symlink;

    let bed = Bed::new("cache-symlink");
    let external = std::env::temp_dir().join(format!(
        "retention-cache-target-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock")
            .as_nanos()
    ));
    fs::create_dir_all(&external).expect("external dir");
    let external_file = external.join("kept.txt");
    fs::write(&external_file, b"outside the journal").expect("external file");
    filetime_set(
        &external,
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800),
    );
    let link = bed.root.join(".cache/cogitate-history/session-link");
    fs::create_dir_all(link.parent().expect("cache root")).expect("cache root");
    symlink(&external, &link).expect("cache link");

    let built = bed.plan(7, "2026-08-05");
    let mine = built.by_class("cogitate_history_cache");
    assert!(mine.is_empty(), "{built:?}");
    assert!(built.retained.iter().any(|kept| {
        kept.rel == ".cache/cogitate-history/session-link" && kept.reason == Kept::NotAMatch
    }));
    assert!(
        solstone_core_journal_io::removal::remove_dir_all(
            &bed.root,
            ".cache/cogitate-history/session-link",
        )
        .is_err(),
        "the contained removal rejects a link that resolves outside the journal"
    );
    assert!(link.exists(), "the link is retained rather than followed");
    assert!(external.is_dir(), "the external directory survives");
    assert!(external_file.is_file(), "the external file survives");

    fs::remove_dir_all(&external).expect("external teardown");
    teardown(&bed);
}

fn filetime_set(path: &std::path::Path, when: std::time::SystemTime) {
    // Set mtime without an extra dependency, via the file's own handle.
    let file = fs::File::open(path).expect("open");
    file.set_times(fs::FileTimes::new().set_modified(when))
        .expect("set mtime");
}

/// ⛔ An undateable entry is kept and named, never assumed old.
#[test]
fn an_undateable_entry_is_kept_and_reported() {
    let bed = Bed::new("undateable");
    bed.file("tokens/notadate.jsonl", b"?");
    bed.file("tokens/20261301.jsonl", b"month 13");
    bed.file("talents/scribe/notanepoch.jsonl", b"?");

    let built = bed.plan(7, "2026-08-05");
    assert!(built.prunable.is_empty(), "{built:?}");
    assert_eq!(
        built
            .retained
            .iter()
            .filter(|kept| kept.reason == Kept::Undateable)
            .count(),
        3,
        "{built:?}"
    );
    teardown(&bed);
}

/// A non-matching extension is not pruned.
#[test]
fn only_the_declared_extensions_are_pruned() {
    let bed = Bed::new("extensions");
    bed.file("tokens/20260101.jsonl", b"prunable");
    bed.file("tokens/20260101.txt", b"not this class");
    bed.file("tokens/20260101", b"no extension");

    let built = bed.plan(7, "2026-08-05");
    assert_eq!(rels(&built.prunable), vec!["tokens/20260101.jsonl"]);
    teardown(&bed);
}

/// 🔴 The property that matters most: a log pass never names owner media.
///
/// One class prunes `chronicle/<day>/health/`, which sits beside the owner's streams.
/// This builds a journal where a real segment holds a real recording, on a day whose
/// health logs ARE prunable, and asserts the plan reaches the log and nothing else.
#[test]
fn a_log_plan_never_names_anything_inside_a_segment() {
    let bed = Bed::new("chronicle-guard");
    bed.file("chronicle/20260101/health/observer.log", b"a log");
    bed.file(
        "chronicle/20260101/field.audio/070000_17/audio.flac",
        b"the owner's recording",
    );
    bed.file(
        "chronicle/20260101/field.audio/070000_17/audio.jsonl",
        b"{}\n",
    );
    // A default-stream segment, directly under the day.
    bed.file("chronicle/20260101/093000_300/audio.flac", b"another");
    // And a segment-shaped file sitting where a log would be.
    bed.file("chronicle/20260101/health/070000_17.jsonl", b"a health log");

    let built = bed.plan(7, "2026-08-05");
    for target in &built.prunable {
        assert!(
            !target.rel().ends_with(".flac"),
            "a log pass named owner media: {}",
            target.rel()
        );
        assert!(
            target.rel().starts_with("chronicle/20260101/health/"),
            "a log pass reached outside a declared class root: {}",
            target.rel()
        );
    }
    assert_eq!(
        rels(&built.prunable),
        vec![
            "chronicle/20260101/health/070000_17.jsonl",
            "chronicle/20260101/health/observer.log",
        ],
        "the health logs, and only those"
    );

    let outcome = remove_logs(&bed.root, &built.prunable);
    assert!(outcome.targets.iter().all(|t| t.not_removed.is_empty()));
    assert!(
        bed.root
            .join("chronicle/20260101/field.audio/070000_17/audio.flac")
            .exists(),
        "the recording survives a log prune"
    );
    assert!(
        bed.root
            .join("chronicle/20260101/093000_300/audio.flac")
            .exists(),
        "and so does the default-stream one"
    );
    teardown(&bed);
}

/// Absent class roots are reported, so an empty plan is explicable.
#[test]
fn absent_classes_are_named_rather_than_silently_skipped() {
    let bed = Bed::new("absent");
    bed.file("tokens/20260101.jsonl", b"old");

    let built = bed.plan(7, "2026-08-05");
    assert_eq!(built.prunable.len(), 1);
    assert_eq!(
        built.absent_classes.len(),
        CLASSES.len() - 1,
        "every class but tokens is absent from this journal: {built:?}"
    );
    assert!(!built.absent_classes.contains(&"tokens"));
    teardown(&bed);
}

/// Every class in the table is reachable, so no row is dead.
#[test]
fn every_class_in_the_table_can_produce_a_target() {
    let bed = Bed::new("every-class");
    bed.file("chronicle/20260101/health/x.log", b"a");
    bed.file("talents/scribe/1767225600000.jsonl", b"a");
    bed.file("talents/20260101.jsonl", b"{\"ts\":1767225600000}\n");
    let cache = bed.dir(".cache/cogitate-history/session");
    filetime_set(
        &cache,
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800),
    );
    bed.file("tokens/20260101.jsonl", b"a");
    bed.file("health/local-inference/20260101.jsonl", b"a");
    bed.file("awareness/20260101.jsonl", b"a");
    bed.file("config/actions/20260101.jsonl", b"a");
    bed.file("facets/work/logs/20260101.jsonl", b"a");
    bed.file("health/pruning-runs/20260101.jsonl", b"a");

    let built = bed.plan(7, "2026-08-05");
    assert!(
        built.absent_classes.is_empty(),
        "every class root exists: {:?}",
        built.absent_classes
    );
    for class in CLASSES {
        assert_eq!(
            built.by_class(class.name).len(),
            1,
            "class `{}` produced no target, so its row is unreachable",
            class.name
        );
    }
    assert_eq!(built.prunable.len(), CLASSES.len());
    assert!(built.bytes() > 0);
    teardown(&bed);
}

/// Planning removes nothing.
#[test]
fn planning_touches_no_file() {
    let bed = Bed::new("read-only");
    let path = bed.file("tokens/20260101.jsonl", b"old");
    let before = fs::metadata(&path).expect("metadata").modified().ok();

    let built = bed.plan(7, "2026-08-05");
    assert_eq!(built.prunable.len(), 1);
    assert!(path.exists(), "planning must not remove");
    assert_eq!(
        fs::metadata(&path).expect("metadata").modified().ok(),
        before,
        "planning must not write"
    );
    teardown(&bed);
}

/// Removal groups by class and reports per class.
#[test]
fn removal_reports_one_row_per_class() {
    let bed = Bed::new("grouped");
    bed.file("tokens/20260101.jsonl", b"a");
    bed.file("tokens/20260102.jsonl", b"a");
    bed.file("awareness/20260101.jsonl", b"a");

    let built = bed.plan(7, "2026-08-05");
    let outcome = remove_logs(&bed.root, &built.prunable);
    assert_eq!(outcome.targets.len(), 2, "two classes: {outcome:?}");
    let tokens = outcome
        .targets
        .iter()
        .find(|target| target.target.dir == "tokens")
        .expect("a tokens row");
    assert_eq!(tokens.removed.len(), 2);
    assert!(outcome.halted.is_none());
    assert!(!bed.root.join("tokens/20260101.jsonl").exists());
    teardown(&bed);
}

/// An already-absent entry is a removal, not a failure -- the pass is idempotent.
#[test]
fn removing_twice_is_not_a_failure() {
    let bed = Bed::new("idempotent");
    bed.file("tokens/20260101.jsonl", b"a");

    let built = bed.plan(7, "2026-08-05");
    let first = remove_logs(&bed.root, &built.prunable);
    assert!(first.targets[0].not_removed.is_empty());
    let second = remove_logs(&bed.root, &built.prunable);
    assert!(
        second.targets[0].not_removed.is_empty(),
        "a stale plan loses entries rather than failing: {second:?}"
    );
    teardown(&bed);
}

// ---------------------------------------------------------------------------
// Append-only logs: compaction, not deletion
// ---------------------------------------------------------------------------

use solstone_core_retention::door::compact_log;
use solstone_core_retention::logs::{COMPACTABLE, plan_compactions};

fn compactions(
    bed: &Bed,
    days: u32,
    today: &str,
) -> Vec<solstone_core_retention::logs::Compaction> {
    plan_compactions(
        &bed.root,
        &LogPolicy {
            days,
            enabled: true,
        },
        date(today),
    )
}

/// 🔴 The second never-pruned log: JSONL despite its `.log` extension.
#[test]
fn the_retention_log_is_compacted_by_its_json_timestamp() {
    let bed = Bed::new("retention-log");
    bed.file(
        "health/retention.log",
        b"{\"timestamp\": \"2026-01-01T00:00:00\", \"files_deleted\": 1}\n\
          {\"timestamp\": \"2026-08-04T00:00:00\", \"files_deleted\": 2}\n",
    );

    let planned = compactions(&bed, 7, "2026-08-05");
    assert_eq!(planned.len(), 1, "{planned:?}");
    assert_eq!(planned[0].name(), "retention_log");
    assert_eq!(planned[0].lines_total, 2);
    assert_eq!(planned[0].lines_dropped, 1);
    assert_eq!(planned[0].undateable_kept, 0);
    assert!(planned[0].bytes_after < planned[0].bytes_before);

    let outcome = compact_log(&bed.root, &planned[0]);
    assert!(outcome.targets[0].not_removed.is_empty(), "{outcome:?}");
    let after = fs::read_to_string(bed.root.join("health/retention.log")).expect("read");
    assert!(!after.contains("2026-01-01"), "the old end is gone");
    assert!(after.contains("2026-08-04"), "the recent end survives");
    assert!(after.ends_with('\n'), "the file stays line-terminated");
    teardown(&bed);
}

/// The root task log's format is different: TSV with a leading epoch.
#[test]
fn the_root_task_log_is_compacted_by_its_leading_epoch() {
    let bed = Bed::new("task-log");
    // 2026-01-01 and 2026-08-04 in epoch seconds.
    bed.file(
        "task_log.txt",
        b"1767225600\tan old task\n1786492800\ta recent task\n",
    );

    let planned = compactions(&bed, 7, "2026-08-05");
    assert_eq!(planned.len(), 1, "{planned:?}");
    assert_eq!(planned[0].name(), "root_task_log");
    assert_eq!(planned[0].lines_dropped, 1);

    let outcome = compact_log(&bed.root, &planned[0]);
    assert!(outcome.targets[0].not_removed.is_empty(), "{outcome:?}");
    let after = fs::read_to_string(bed.root.join("task_log.txt")).expect("read");
    assert_eq!(after, "1786492800\ta recent task\n");
    teardown(&bed);
}

/// ⛔ An undateable line is KEPT and counted, never dropped.
#[test]
fn an_undateable_line_survives_compaction_and_is_counted() {
    let bed = Bed::new("undateable-lines");
    bed.file(
        "task_log.txt",
        b"not a dated line at all\n\
          1767225600\tan old task\n\
          \tno epoch but a tab\n\
          also-not-dated\n\
          1767225600\n",
    );

    let planned = compactions(&bed, 7, "2026-08-05");
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].lines_dropped, 1, "only the dated old line goes");
    // ⛔ The last line is a VALID epoch with no tab. A reader that did not require
    // the separator would date it and drop it, silently truncating an undated log.
    assert_eq!(planned[0].undateable_kept, 4);

    let performed = compact_log(&bed.root, &planned[0]);
    assert!(
        performed.targets[0].not_removed.is_empty(),
        "the compaction must succeed for this assertion to mean anything: {performed:?}"
    );
    let after = fs::read_to_string(bed.root.join("task_log.txt")).expect("read");
    assert!(after.contains("not a dated line at all"));
    assert!(after.contains("no epoch but a tab"));
    assert!(after.contains("also-not-dated"));
    assert!(
        after.contains("1767225600\n"),
        "a tab-less line must survive even when it parses as an epoch"
    );
    assert!(!after.contains("an old task"));
    teardown(&bed);
}

/// ⛔ A malformed JSON line in the retention log is kept, not dropped.
#[test]
fn a_malformed_json_line_is_kept() {
    let bed = Bed::new("malformed-json");
    bed.file(
        "health/retention.log",
        b"{not json at all\n\
          {\"no_timestamp\": true}\n\
          {\"timestamp\": \"2026-01-01T00:00:00\"}\n",
    );

    let planned = compactions(&bed, 7, "2026-08-05");
    assert_eq!(planned[0].lines_dropped, 1);
    assert_eq!(planned[0].undateable_kept, 2);
    teardown(&bed);
}

/// A log with nothing to drop is not rewritten at all.
#[test]
fn a_log_with_nothing_old_is_left_untouched() {
    let bed = Bed::new("no-op");
    let path = bed.file("task_log.txt", b"1786492800\ta recent task\n");
    let before = fs::metadata(&path).expect("metadata").modified().ok();

    assert!(
        compactions(&bed, 7, "2026-08-05").is_empty(),
        "a no-op compaction must not be proposed, or it resets the mtime for nothing"
    );
    assert_eq!(
        fs::metadata(&path).expect("metadata").modified().ok(),
        before
    );
    teardown(&bed);
}

/// Planning a compaction touches nothing.
#[test]
fn planning_a_compaction_does_not_rewrite() {
    let bed = Bed::new("plan-only");
    let path = bed.file(
        "task_log.txt",
        b"1767225600\tan old task\n1786492800\ta recent task\n",
    );
    let planned = compactions(&bed, 7, "2026-08-05");
    assert_eq!(planned.len(), 1);
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        "1767225600\tan old task\n1786492800\ta recent task\n",
        "the plan is a value; nothing is written until it is performed"
    );
    teardown(&bed);
}

/// ⛔ The file's existing mode survives compaction.
#[test]
fn compaction_preserves_a_tightened_mode() {
    use std::os::unix::fs::PermissionsExt;

    let bed = Bed::new("mode");
    let path = bed.file(
        "task_log.txt",
        b"1767225600\tan old task\n1786492800\ta recent task\n",
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");

    let planned = compactions(&bed, 7, "2026-08-05");
    let performed = compact_log(&bed.root, &planned[0]);
    assert!(
        performed.targets[0].not_removed.is_empty(),
        "the compaction must succeed for this assertion to mean anything: {performed:?}"
    );
    assert_eq!(
        fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
        0o600,
        "a log the operator tightened must not be widened by being pruned"
    );
    teardown(&bed);
}

/// A disabled or unset window compacts nothing.
#[test]
fn a_disabled_window_compacts_nothing() {
    let bed = Bed::new("disabled-compact");
    bed.file("task_log.txt", b"1767225600\tan old task\n");
    assert!(plan_compactions(&bed.root, &LogPolicy::default(), date("2026-08-05")).is_empty());
    assert!(
        plan_compactions(
            &bed.root,
            &LogPolicy {
                days: 0,
                enabled: true
            },
            date("2026-08-05")
        )
        .is_empty(),
        "days: 0 must not empty the journal's task log"
    );
    teardown(&bed);
}

/// An absent log is not a failure.
#[test]
fn an_absent_append_only_log_is_skipped() {
    let bed = Bed::new("absent-compact");
    assert!(compactions(&bed, 7, "2026-08-05").is_empty());
    teardown(&bed);
}

/// Both append-only logs are reachable, so neither row is dead.
#[test]
fn every_compactable_log_can_produce_a_plan() {
    let bed = Bed::new("both-compact");
    bed.file("task_log.txt", b"1767225600\tan old task\n");
    bed.file(
        "health/retention.log",
        b"{\"timestamp\": \"2026-01-01T00:00:00\"}\n",
    );

    let planned = compactions(&bed, 7, "2026-08-05");
    assert_eq!(planned.len(), COMPACTABLE.len(), "{planned:?}");
    for log in COMPACTABLE {
        assert!(
            planned.iter().any(|done| done.name() == log.name),
            "`{}` produced no plan, so its row is unreachable",
            log.name
        );
    }
    teardown(&bed);
}
